//! Wire-level regression fixtures: exercise `SQLx` type checks without a live database.
use crate::{CancellationToken, Connection, ConnectionProfile, DbDriverType, DbValue};
use std::{io, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

async fn packet(stream: &mut TcpStream, sequence: u8, data: &[u8]) -> io::Result<()> {
    let length = u32::try_from(data.len())
        .map_err(io::Error::other)?
        .to_le_bytes();
    stream
        .write_all(&[length[0], length[1], length[2], sequence])
        .await?;
    stream.write_all(data).await
}

async fn read_packet(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut header = [0; 4];
    stream.read_exact(&mut header).await?;
    let length = u32::from_le_bytes([header[0], header[1], header[2], 0]);
    let mut data = vec![0; usize::try_from(length).map_err(io::Error::other)?];
    stream.read_exact(&mut data).await?;
    Ok(data)
}

fn field(buffer: &mut Vec<u8>, value: &[u8]) -> io::Result<()> {
    buffer.push(u8::try_from(value.len()).map_err(io::Error::other)?);
    buffer.extend(value);
    Ok(())
}

const VALUES: [[Option<&str>; 4]; 3] = [
    [
        Some("2026-11-02 11:00:36.123456"),
        Some("2026-11-02 14:00:36.000001"),
        Some("2026-11-02"),
        Some("11:00:36.123456"),
    ],
    [None, None, None, None],
    [
        Some("0000-00-00 00:00:00"),
        Some("0000-00-00 00:00:00"),
        Some("0000-00-00"),
        Some("-838:59:59.000001"),
    ],
];

async fn result_set(stream: &mut TcpStream) -> io::Result<()> {
    packet(stream, 1, &[4]).await?;
    let mut sequence = 2;
    // Native MySQL field codes, not VARCHAR aliases: reproduce the screenshot.
    for (name, kind) in [
        ("created_time", 0x07),
        ("valid_time", 0x0c),
        ("date", 0x0a),
        ("duration", 0x0b),
    ] {
        let mut column = Vec::new();
        for part in ["def", "fixture", "goods", "goods", name, name] {
            field(&mut column, part.as_bytes())?;
        }
        column.extend([0x0c, 45, 0]); // fixed fields length and utf8mb4 collation
        column.extend(64_u32.to_le_bytes());
        column.extend([kind, 0, 0, 6, 0, 0]); // native type, nullable flags, microseconds
        packet(stream, sequence, &column).await?;
        sequence += 1;
    }
    packet(stream, sequence, &[0xfe, 0, 0, 2, 0]).await?;
    sequence += 1;
    for row in VALUES {
        let mut data = Vec::new();
        for value in row {
            if let Some(value) = value {
                field(&mut data, value.as_bytes())?;
            } else {
                data.push(0xfb); // SQL NULL
            }
        }
        packet(stream, sequence, &data).await?;
        sequence += 1;
    }
    packet(stream, sequence, &[0xfe, 0, 0, 2, 0]).await
}

async fn serve(listener: TcpListener) -> io::Result<()> {
    let (mut stream, _) = listener.accept().await?;
    let mut hello = b"\x0a8.0.0-fixture\0".to_vec();
    hello.extend(1_u32.to_le_bytes());
    hello.extend(b"12345678\0");
    hello.extend(0x8201_u16.to_le_bytes()); // protocol 4.1, secure connection, MySQL
    hello.push(45);
    hello.extend(2_u16.to_le_bytes()); // autocommit
    hello.extend([0; 13]); // upper capabilities, auth length and reserved bytes
    hello.extend(b"abcdefghijkl\0");
    packet(&mut stream, 0, &hello).await?;
    read_packet(&mut stream).await?;
    packet(&mut stream, 2, &[0, 0, 0, 2, 0, 0, 0]).await?;
    loop {
        let command = match read_packet(&mut stream).await {
            Ok(command) => command,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        };
        match command.first() {
            Some(0x01) => return Ok(()), // COM_QUIT
            Some(0x03) if command.get(1..) == Some(b"SELECT temporal_fixture") => {
                result_set(&mut stream).await?;
            }
            Some(0x16) => {
                // Describe is optional; this fixture serves only COM_QUERY.
                packet(
                    &mut stream,
                    1,
                    b"\xff\x28\x04#HY000Prepare unavailable in fixture",
                )
                .await?;
            }
            Some(0x03 | 0x0e) => {
                // session initialization / ping
                packet(&mut stream, 1, &[0, 0, 0, 2, 0, 0, 0]).await?;
            }
            _ => return Err(io::Error::other("Unexpected fixture command")),
        }
    }
}

#[tokio::test]
async fn mysql_temporal_wire_values_preserve_session_time_nulls_and_zero_dates()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let profile = ConnectionProfile {
            name: "temporal fixture".into(),
            driver: DbDriverType::Mysql,
            host: "127.0.0.1".into(),
            port: listener.local_addr()?.port(),
            user: "fixture".into(),
            database: "fixture".into(),
            ..ConnectionProfile::default()
        };
        let server = tokio::spawn(serve(listener));
        let db = Connection::connect(&profile, None, None).await?;
        let results = db
            .query("SELECT temporal_fixture", None, &CancellationToken::new())
            .await?;
        let result = results.first().ok_or("missing result")?;
        assert_eq!(
            result
                .columns
                .iter()
                .map(|c| c.data_type.as_str())
                .collect::<Vec<_>>(),
            ["TIMESTAMP", "DATETIME", "DATE", "TIME"]
        );
        assert_eq!(result.rows.len(), VALUES.len());
        for (row, expected) in result.rows.iter().zip(VALUES) {
            for (value, expected) in row.iter().zip(expected) {
                assert_eq!(
                    value,
                    &expected.map_or(DbValue::Null, |s| DbValue::DateTime(s.into()))
                );
            }
        }
        assert!(!result.truncated);
        db.close().await;
        server.await??;
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await?
}
