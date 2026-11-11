use super::{Atom, Data, Error, KeyRow, Mutation, Profile, Result, ScanPage, Snapshot};
use ::redis::{AsyncCommands as _, aio::MultiplexedConnection};
use nozhik_remote_ssh::{SshSession, credentials::Secret, tunnel::TunnelHandle};
use std::time::Duration;

pub(super) const MAX_BYTES: usize = 1024 * 1024;
const MAX_ITEMS: usize = 1000;
/// Redis endpoint owning its verified SSH tunnel. Each operation gets a dedicated socket.
pub struct Connection {
    client: ::redis::Client,
    tunnel: Option<TunnelHandle>,
}
impl Connection {
    /// Authenticate, SELECT the profile database and check PING.
    /// # Errors
    /// Reports endpoint, authentication, database and SSH failures.
    pub async fn connect(
        profile: &Profile,
        password: Option<&Secret>,
        ssh: Option<&SshSession>,
    ) -> Result<Self> {
        profile.validate()?;
        let tunnel = if profile.ssh_profile_id.is_some() {
            Some(
                ssh.ok_or_else(|| Error::Invalid("Verified SSH session required".into()))?
                    .start_tunnel(0, &profile.host, profile.port)
                    .await?,
            )
        } else {
            None
        };
        let (host, port) = tunnel.as_ref().map_or_else(
            || (profile.host.clone(), profile.port),
            |t| ("127.0.0.1".into(), t.local_addr().port()),
        );
        let info = ::redis::ConnectionInfo {
            addr: ::redis::ConnectionAddr::Tcp(host, port),
            redis: ::redis::RedisConnectionInfo {
                db: i64::from(profile.database),
                username: (!profile.user.is_empty()).then(|| profile.user.clone()),
                password: password
                    .filter(|p| !p.expose().is_empty())
                    .map(|p| p.expose().into()),
                ..Default::default()
            },
        };
        let connection = Self {
            client: ::redis::Client::open(info)?,
            tunnel,
        };
        let mut socket = connection.socket(profile.database).await?;
        let _: String = ::redis::cmd("PING").query_async(&mut socket).await?;
        // Cluster requires redirects and node-specific transport. Never present a partial cluster scan.
        let cluster: ::redis::RedisResult<String> = ::redis::cmd("INFO")
            .arg("cluster")
            .query_async(&mut socket)
            .await;
        if cluster.is_ok_and(|v| v.lines().any(|l| l.trim() == "cluster_enabled:1")) {
            return Err(Error::Invalid(
                "Redis Cluster is not supported yet; use a standalone Redis endpoint".into(),
            ));
        }
        Ok(connection)
    }
    async fn socket(&self, database: u32) -> Result<MultiplexedConnection> {
        if self.tunnel.as_ref().is_some_and(|t| !t.is_running()) {
            return Err(Error::Invalid("Redis SSH tunnel disconnected".into()));
        }
        let mut c = self
            .client
            .get_multiplexed_async_connection_with_config(
                &::redis::AsyncConnectionConfig::new()
                    .set_connection_timeout(Duration::from_secs(15))
                    .set_response_timeout(Duration::from_secs(15)),
            )
            .await?;
        let _: () = ::redis::cmd("SELECT")
            .arg(database)
            .query_async(&mut c)
            .await?;
        Ok(c)
    }
    /// Read one key-name cursor step using literal substring matching.
    /// # Errors
    /// Reports connection errors; never executes KEYS.
    pub async fn scan(&self, database: u32, cursor: u64, substring: &str) -> Result<ScanPage> {
        let mut c = self.socket(database).await?;
        let pattern = substring_pattern(substring);
        let (cursor, keys): (u64, Vec<Vec<u8>>) = ::redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(100)
            .query_async(&mut c)
            .await?;
        let mut rows = Vec::new();
        for key in keys {
            let (kind, ttl_ms): (String, i64) = ::redis::pipe()
                .cmd("TYPE")
                .arg(&key)
                .cmd("PTTL")
                .arg(&key)
                .query_async(&mut c)
                .await?;
            if kind == "none" || ttl_ms == -2 {
                continue;
            }
            let preview = match kind.as_str() {
                "string" => {
                    let v: Vec<u8> = ::redis::cmd("GETRANGE")
                        .arg(&key)
                        .arg(0)
                        .arg(255)
                        .query_async(&mut c)
                        .await?;
                    Atom::from_bytes(v).preview()
                }
                "hash" | "list" | "set" | "zset" | "stream" => {
                    let cmd = match kind.as_str() {
                        "hash" => "HLEN",
                        "list" => "LLEN",
                        "set" => "SCARD",
                        "zset" => "ZCARD",
                        _ => "XLEN",
                    };
                    let n: u64 = ::redis::cmd(cmd).arg(&key).query_async(&mut c).await?;
                    format!(
                        "{n} elements · {}",
                        collection_preview(&mut c, &key, &kind).await?
                    )
                }
                _ => "Open for type information".into(),
            };
            rows.push(KeyRow {
                key: Atom::from_bytes(key),
                kind,
                preview,
                ttl_ms,
            });
        }
        Ok(ScanPage { cursor, rows })
    }
    /// Load at most 1000 entries for the editor; large values stay read-only.
    /// # Errors
    /// Reports missing keys, concurrent changes and Redis errors.
    #[allow(
        clippy::too_many_lines,
        reason = "One WATCH-scoped read decodes six Redis structures and validates a coherent baseline"
    )]
    pub async fn read(&self, database: u32, key: &Atom) -> Result<Snapshot> {
        let mut c = self.socket(database).await?;
        let key_bytes = key.bytes()?;
        let _: () = ::redis::cmd("WATCH")
            .arg(&key_bytes)
            .query_async(&mut c)
            .await?;
        let (kind, ttl_ms): (String, i64) = ::redis::pipe()
            .cmd("TYPE")
            .arg(&key_bytes)
            .cmd("PTTL")
            .arg(&key_bytes)
            .query_async(&mut c)
            .await?;
        let mut truncated = false;
        let data = match kind.as_str() {
            "none" => {
                return Err(Error::Invalid(
                    "Key expired or was deleted; refresh the search".into(),
                ));
            }
            "string" => {
                let size: usize = c.strlen(&key_bytes).await?;
                truncated = size > MAX_BYTES;
                let bytes: Vec<u8> = c
                    .getrange(
                        &key_bytes,
                        0,
                        isize::try_from(MAX_BYTES - 1)
                            .map_err(|e| Error::Invalid(e.to_string()))?,
                    )
                    .await?;
                Data::String(Atom::from_bytes(bytes))
            }
            "list" => {
                let size: usize = c.llen(&key_bytes).await?;
                truncated = size > MAX_ITEMS;
                let values: Vec<Vec<u8>> = c.lrange(&key_bytes, 0, 999).await?;
                Data::List(values.into_iter().map(Atom::from_bytes).collect())
            }
            "hash" | "set" | "zset" => {
                read_collection(&mut c, &key_bytes, &kind, &mut truncated).await?
            }
            "stream" => {
                let size: usize = ::redis::cmd("XLEN")
                    .arg(&key_bytes)
                    .query_async(&mut c)
                    .await?;
                truncated = size > MAX_ITEMS;
                let records: Vec<(String, Vec<Vec<u8>>)> = ::redis::cmd("XRANGE")
                    .arg(&key_bytes)
                    .arg("-")
                    .arg("+")
                    .arg("COUNT")
                    .arg(MAX_ITEMS)
                    .query_async(&mut c)
                    .await?;
                Data::Stream(
                    records
                        .into_iter()
                        .map(|(id, fields)| {
                            (
                                id,
                                fields
                                    .as_chunks::<2>()
                                    .0
                                    .iter()
                                    .map(|p| {
                                        (
                                            Atom::from_bytes(p[0].clone()),
                                            Atom::from_bytes(p[1].clone()),
                                        )
                                    })
                                    .collect(),
                            )
                        })
                        .collect(),
                )
            }
            _ => Data::Unsupported(kind),
        };
        let json = serde_json::to_vec(&data)?;
        if json.len() > MAX_BYTES {
            return Err(Error::Invalid(
                "Value exceeds the 1 MiB editor limit; use a dedicated Redis client for this key"
                    .into(),
            ));
        }
        let dump = if truncated || matches!(data, Data::Unsupported(_)) {
            Vec::new()
        } else {
            ::redis::cmd("DUMP")
                .arg(&key_bytes)
                .query_async(&mut c)
                .await?
        };
        let stable: Option<(String,)> = ::redis::pipe()
            .atomic()
            .cmd("PING")
            .query_async(&mut c)
            .await?;
        if stable.is_none() {
            return Err(Error::Invalid(
                "Key changed while loading; reload it".into(),
            ));
        }
        Ok(Snapshot {
            key: key.clone(),
            data,
            dump,
            ttl_ms,
            truncated,
        })
    }
    /// Apply an optimistic operation, checking the complete original value atomically.
    /// # Errors
    /// Rejects stale, truncated and unsupported snapshots. A lost reply may mean a write succeeded.
    pub async fn mutate(
        &self,
        database: u32,
        snapshot: &Snapshot,
        change: &Mutation,
    ) -> Result<()> {
        if snapshot.truncated || snapshot.dump.is_empty() {
            return Err(Error::Invalid(
                "Partial and unsupported values are read-only".into(),
            ));
        }
        let (operation, args) = mutation_arguments(&snapshot.data, change)?;
        let mut c = self.socket(database).await?;
        let script = ::redis::Script::new(MUTATE);
        let mut invocation = script.prepare_invoke();
        invocation
            .key(snapshot.key.bytes()?)
            .arg(&snapshot.dump)
            .arg(operation)
            .arg(snapshot.data.kind());
        for arg in args {
            invocation.arg(arg);
        }
        let _: i64 = invocation.invoke_async(&mut c).await?;
        Ok(())
    }
    /// Create a new key without overwriting an existing value.
    /// # Errors
    /// Reports invalid data, an existing key or Redis errors.
    pub async fn create(&self, database: u32, key: &Atom, data: &Data) -> Result<()> {
        let args = data.arguments()?;
        let mut c = self.socket(database).await?;
        let script = ::redis::Script::new(MUTATE);
        let mut invocation = script.prepare_invoke();
        invocation
            .key(key.bytes()?)
            .arg(Vec::<u8>::new())
            .arg("create")
            .arg(data.kind());
        for arg in args {
            invocation.arg(arg);
        }
        let _: i64 = invocation.invoke_async(&mut c).await?;
        Ok(())
    }
}
async fn collection_preview(
    c: &mut MultiplexedConnection,
    key: &[u8],
    kind: &str,
) -> Result<String> {
    let values: Vec<Vec<u8>> = match kind {
        "list" => {
            ::redis::cmd("LRANGE")
                .arg(key)
                .arg(0)
                .arg(2)
                .query_async(c)
                .await?
        }
        "hash" | "set" => {
            let (_, values): (u64, Vec<Vec<u8>>) =
                ::redis::cmd(if kind == "hash" { "HSCAN" } else { "SSCAN" })
                    .arg(key)
                    .arg(0)
                    .arg("COUNT")
                    .arg(3)
                    .query_async(c)
                    .await?;
            values
        }
        "zset" => {
            ::redis::cmd("ZRANGE")
                .arg(key)
                .arg(0)
                .arg(2)
                .arg("WITHSCORES")
                .query_async(c)
                .await?
        }
        _ => {
            let rows: Vec<(String, Vec<Vec<u8>>)> = ::redis::cmd("XRANGE")
                .arg(key)
                .arg("-")
                .arg("+")
                .arg("COUNT")
                .arg(1)
                .query_async(c)
                .await?;
            return Ok(rows
                .into_iter()
                .next()
                .map_or_else(String::new, |(id, fields)| {
                    format!(
                        "{id}: {}",
                        fields
                            .into_iter()
                            .take(4)
                            .map(|v| Atom::from_bytes(v).preview())
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                }));
        }
    };
    Ok(values
        .into_iter()
        .take(6)
        .map(|v| Atom::from_bytes(v).preview())
        .collect::<Vec<_>>()
        .join(" | ")
        .chars()
        .take(256)
        .collect())
}
pub(super) fn substring_pattern(text: &str) -> String {
    let mut pattern = String::from("*");
    for ch in text.chars() {
        if matches!(ch, '*' | '?' | '[' | ']' | '\\') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('*');
    pattern
}
async fn read_collection(
    c: &mut MultiplexedConnection,
    key: &[u8],
    kind: &str,
    truncated: &mut bool,
) -> Result<Data> {
    let cmd = match kind {
        "hash" => "HSCAN",
        "set" => "SSCAN",
        _ => "ZSCAN",
    };
    let mut cursor = 0;
    let mut members = std::collections::BTreeMap::new();
    let mut bytes = 0;
    loop {
        let (next, values): (u64, Vec<Vec<u8>>) = ::redis::cmd(cmd)
            .arg(key)
            .arg(cursor)
            .arg("COUNT")
            .arg(100)
            .query_async(c)
            .await?;
        let width = if kind == "set" { 1 } else { 2 };
        for pair in values.chunks_exact(width) {
            bytes += pair.iter().map(Vec::len).sum::<usize>();
            if bytes > MAX_BYTES || members.len() >= MAX_ITEMS {
                *truncated = true;
                break;
            }
            members.insert(pair[0].clone(), pair.get(1).cloned().unwrap_or_default());
        }
        cursor = next;
        if *truncated || cursor == 0 {
            break;
        }
    }
    Ok(match kind {
        "hash" => Data::Hash(
            members
                .into_iter()
                .map(|(k, v)| (Atom::from_bytes(k), Atom::from_bytes(v)))
                .collect(),
        ),
        "set" => Data::Set(members.into_keys().map(Atom::from_bytes).collect()),
        _ => Data::Zset(
            members
                .into_iter()
                .map(|(k, v)| {
                    (
                        Atom::from_bytes(k),
                        String::from_utf8_lossy(&v).into_owned(),
                    )
                })
                .collect(),
        ),
    })
}
fn mutation_arguments(data: &Data, change: &Mutation) -> Result<(&'static str, Vec<Vec<u8>>)> {
    Ok(match change {
        Mutation::Replace(replacement) => {
            if replacement.kind() != data.kind() {
                return Err(Error::Invalid(
                    "Changing the structure type is not supported".into(),
                ));
            }
            ("replace", replacement.arguments()?)
        }
        Mutation::Delete => ("delete", Vec::new()),
        Mutation::Expire(seconds) => {
            if seconds.is_some_and(|n| n == 0 || n > 2_147_483_647) {
                return Err(Error::Invalid(
                    "TTL must be 1..2147483647 seconds, or persistent".into(),
                ));
            }
            (
                "expire",
                vec![
                    seconds
                        .map_or_else(|| "-1".into(), |n| n.to_string())
                        .into_bytes(),
                ],
            )
        }
        Mutation::Append(fields) => {
            if !matches!(data, Data::Stream(_)) {
                return Err(Error::Invalid("Select a Stream first".into()));
            }
            ("append", Data::Hash(fields.clone()).arguments()?)
        }
        Mutation::DeleteEntry(id) => {
            if !matches!(data, Data::Stream(rows) if rows.iter().any(|(i,_)| i == id)) {
                return Err(Error::Invalid("Select an existing Stream ID".into()));
            }
            ("entry", vec![id.as_bytes().to_vec()])
        }
    })
}
// Arguments are validated on the client before invoking this bounded script. Redis scripts
// provide isolation, not rollback on server failures; never retry a write automatically.
const MUTATE: &str = r"
local key = KEYS[1]
local current = redis.call('DUMP', key)
if ARGV[2] == 'create' then
    if current then return redis.error_reply('Key already exists; reload it') end
elseif not current or current ~= ARGV[1] then
    return redis.error_reply('Key changed or expired; reload before applying changes')
end
local op = ARGV[2]
if op == 'delete' then return redis.call('DEL', key) end
if op == 'expire' then
    if ARGV[4] == '-1' then return redis.call('PERSIST', key) end
    return redis.call('EXPIRE', key, ARGV[4])
end
if op == 'append' then redis.call('XADD', key, '*', unpack(ARGV, 4)); return 1 end
if op == 'entry' then return redis.call('XDEL', key, ARGV[4]) end
local ttl = redis.call('PTTL', key)
local kind = ARGV[3]
local command = ({string='SET', list='RPUSH', set='SADD', hash='HSET', zset='ZADD'})[kind]
if not command then return redis.error_reply('Unsupported structure') end
local clock = redis.call('TIME')
local staging = key .. '\0nozhik:' .. clock[1] .. ':' .. clock[2]
if redis.call('EXISTS', staging) ~= 0 then return redis.error_reply('Temporary key collision; retry after reloading') end
if redis.acl_check_cmd then
    if not redis.acl_check_cmd(command, staging, unpack(ARGV, 4)) or
       not redis.acl_check_cmd('DEL', staging) or
       not redis.acl_check_cmd('PEXPIRE', staging, 1) or
       not redis.acl_check_cmd('PERSIST', staging) or
       not redis.acl_check_cmd('RENAME', staging, key) then
        return redis.error_reply('Redis ACL denies the guarded replacement commands')
    end
end
local field_ttls = {}
if kind == 'hash' and op ~= 'create' then
    local fields = {}
    for i = 4, #ARGV, 2 do fields[#fields + 1] = ARGV[i] end
    local result = redis.pcall('HPTTL', key, 'FIELDS', #fields, unpack(fields))
    if type(result) == 'table' and result.err then
        if not string.find(string.lower(result.err), 'unknown', 1, true) then return result end
    else field_ttls = result end
end
local built = redis.pcall(command, staging, unpack(ARGV, 4))
if type(built) == 'table' and built.err then return built end
for index, remaining in ipairs(field_ttls) do
    if remaining >= 0 then
        local field_expiry = redis.pcall('HPEXPIRE', staging, math.max(remaining, 1), 'FIELDS', 1, ARGV[index * 2 + 2])
        if type(field_expiry) == 'table' and field_expiry.err then redis.pcall('DEL', staging); return field_expiry end
    end
end
local expiry
if ttl >= 0 then expiry = redis.pcall('PEXPIRE', staging, math.max(ttl, 1))
else expiry = redis.pcall('PERSIST', staging) end
if type(expiry) == 'table' and expiry.err then redis.pcall('DEL', staging); return expiry end
local renamed = redis.pcall('RENAME', staging, key)
if type(renamed) == 'table' and renamed.err then redis.pcall('DEL', staging); return renamed end
return 1
";
