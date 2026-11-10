//! Built-in Go functions and the standard packages used by completion and hover.
//! The catalog is the common surface (`fmt`, `net/http`, `testing`, and the
//! predeclared identifiers). It is not the whole standard library.

use crate::{
    doc_provider::{DefinitionTarget, EntityKind, LanguageDocProvider, SymbolDocInfo},
    lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind},
};

/// One function, method, or field.
#[derive(Clone, Copy)]
pub struct Item {
    /// Identifier.
    pub name: &'static str,
    /// Signature shown in the popup.
    pub signature: &'static str,
    /// Short documentation.
    pub documentation: &'static str,
    /// Result type identity, or empty when the result is not a known type.
    pub returns: &'static str,
    /// True for a field rather than a callable.
    pub field: bool,
}

/// A standard package.
#[derive(Clone, Copy)]
pub struct Package {
    /// Import path.
    pub path: &'static str,
    /// Default alias, the last path element.
    pub alias: &'static str,
    /// Package summary.
    pub documentation: &'static str,
    /// Functions and constants.
    pub items: &'static [Item],
    /// Named types.
    pub types: &'static [Type],
}

/// A named standard type.
#[derive(Clone, Copy)]
pub struct Type {
    /// Type name without the package.
    pub name: &'static str,
    /// Type summary.
    pub documentation: &'static str,
    /// Methods and fields.
    pub members: &'static [Item],
}

const fn item(
    name: &'static str,
    signature: &'static str,
    documentation: &'static str,
    returns: &'static str,
) -> Item {
    Item {
        name,
        signature,
        documentation,
        returns,
        field: false,
    }
}

const fn field(
    name: &'static str,
    signature: &'static str,
    documentation: &'static str,
    returns: &'static str,
) -> Item {
    Item {
        name,
        signature,
        documentation,
        returns,
        field: true,
    }
}

const ERROR: &[Item] = &[item(
    "Error",
    "func (error) Error() string",
    "Returns the error text.",
    "builtin::string",
)];

/// Predeclared functions.
pub const BUILTINS: &[Item] = &[
    item(
        "append",
        "func append(slice []T, elems ...T) []T",
        "Appends elements to a slice.",
        "",
    ),
    item(
        "cap",
        "func cap(v Type) int",
        "Capacity of a slice, array, channel, or pointer to an array.",
        "builtin::int",
    ),
    item(
        "clear",
        "func clear(v Type)",
        "Clears a map, slice, or zeroable value.",
        "",
    ),
    item(
        "close",
        "func close(c chan<- Type)",
        "Closes a channel.",
        "",
    ),
    item(
        "complex",
        "func complex(r, i Float) Complex",
        "Builds a complex number.",
        "",
    ),
    item(
        "copy",
        "func copy(dst, src []T) int",
        "Copies elements between slices.",
        "builtin::int",
    ),
    item(
        "delete",
        "func delete(m map[K]V, key K)",
        "Removes a map key.",
        "",
    ),
    item(
        "imag",
        "func imag(c Complex) Float",
        "Imaginary part of a complex number.",
        "",
    ),
    item(
        "len",
        "func len(v Type) int",
        "Length of a string, slice, array, map, or channel.",
        "builtin::int",
    ),
    item(
        "make",
        "func make(t Type, size ...IntegerType) Type",
        "Creates a slice, map, or channel.",
        "",
    ),
    item(
        "max",
        "func max[T cmp.Ordered](x T, y ...T) T",
        "Returns the largest argument.",
        "",
    ),
    item(
        "min",
        "func min[T cmp.Ordered](x T, y ...T) T",
        "Returns the smallest argument.",
        "",
    ),
    item(
        "new",
        "func new(Type) *Type",
        "Allocates a zero value and returns its address.",
        "",
    ),
    item(
        "panic",
        "func panic(v any)",
        "Stops the current goroutine.",
        "",
    ),
    item(
        "print",
        "func print(args ...Type)",
        "Writes arguments to stderr. Prefer fmt or log.",
        "",
    ),
    item(
        "println",
        "func println(args ...Type)",
        "Writes arguments and a newline to stderr.",
        "",
    ),
    item(
        "real",
        "func real(c Complex) Float",
        "Real part of a complex number.",
        "",
    ),
    item(
        "recover",
        "func recover() any",
        "Stops a panic inside a deferred function.",
        "builtin::any",
    ),
];

/// Keywords offered as completion when the caret is not after a dot.
pub const KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];

/// Predeclared constants and types that complete as identifiers.
pub const CONSTANTS: &[Item] = &[
    item(
        "nil",
        "nil",
        "The zero value for pointers, interfaces, maps, slices, channels, and functions.",
        "",
    ),
    item("true", "true", "Boolean true.", "builtin::bool"),
    item("false", "false", "Boolean false.", "builtin::bool"),
    item(
        "iota",
        "iota",
        "Integer counter inside a const block.",
        "builtin::int",
    ),
];

const FMT: &[Item] = &[
    item(
        "Errorf",
        "func Errorf(format string, a ...any) error",
        "Formats an error.",
        "builtin::error",
    ),
    item(
        "Print",
        "func Print(a ...any) (n int, err error)",
        "Formats with default formats.",
        "builtin::int",
    ),
    item(
        "Printf",
        "func Printf(format string, a ...any) (n int, err error)",
        "Formats according to a format string.",
        "builtin::int",
    ),
    item(
        "Println",
        "func Println(a ...any) (n int, err error)",
        "Formats with spaces and a newline.",
        "builtin::int",
    ),
    item(
        "Sprint",
        "func Sprint(a ...any) string",
        "Returns the default formatting.",
        "builtin::string",
    ),
    item(
        "Sprintf",
        "func Sprintf(format string, a ...any) string",
        "Returns a formatted string.",
        "builtin::string",
    ),
    item(
        "Sprintln",
        "func Sprintln(a ...any) string",
        "Returns spaced arguments and a newline.",
        "builtin::string",
    ),
    item(
        "Fprint",
        "func Fprint(w io.Writer, a ...any) (n int, err error)",
        "Writes default formatting to w.",
        "builtin::int",
    ),
    item(
        "Fprintf",
        "func Fprintf(w io.Writer, format string, a ...any) (n int, err error)",
        "Writes formatted text to w.",
        "builtin::int",
    ),
    item(
        "Fprintln",
        "func Fprintln(w io.Writer, a ...any) (n int, err error)",
        "Writes spaced arguments and a newline to w.",
        "builtin::int",
    ),
];

const ERRORS: &[Item] = &[
    item(
        "New",
        "func New(text string) error",
        "Returns an error with the given text.",
        "builtin::error",
    ),
    item(
        "Is",
        "func Is(err, target error) bool",
        "Reports whether the chain matches target.",
        "builtin::bool",
    ),
    item(
        "As",
        "func As(err error, target any) bool",
        "Finds the first error matching target.",
        "builtin::bool",
    ),
    item(
        "Unwrap",
        "func Unwrap(err error) error",
        "Returns the next error in the chain.",
        "builtin::error",
    ),
    item(
        "Join",
        "func Join(errs ...error) error",
        "Combines errors.",
        "builtin::error",
    ),
];

const STRINGS: &[Item] = &[
    item(
        "Contains",
        "func Contains(s, substr string) bool",
        "Reports whether substr is in s.",
        "builtin::bool",
    ),
    item(
        "HasPrefix",
        "func HasPrefix(s, prefix string) bool",
        "Reports whether s starts with prefix.",
        "builtin::bool",
    ),
    item(
        "HasSuffix",
        "func HasSuffix(s, suffix string) bool",
        "Reports whether s ends with suffix.",
        "builtin::bool",
    ),
    item(
        "Join",
        "func Join(elems []string, sep string) string",
        "Joins elements with a separator.",
        "builtin::string",
    ),
    item(
        "Split",
        "func Split(s, sep string) []string",
        "Splits s around sep.",
        "",
    ),
    item(
        "Replace",
        "func Replace(s, old, new string, n int) string",
        "Replaces the first n copies.",
        "builtin::string",
    ),
    item(
        "ReplaceAll",
        "func ReplaceAll(s, old, new string) string",
        "Replaces every copy.",
        "builtin::string",
    ),
    item(
        "Trim",
        "func Trim(s, cutset string) string",
        "Removes leading and trailing cutset runes.",
        "builtin::string",
    ),
    item(
        "TrimSpace",
        "func TrimSpace(s string) string",
        "Removes leading and trailing Unicode spaces.",
        "builtin::string",
    ),
    item(
        "ToLower",
        "func ToLower(s string) string",
        "Returns s mapped to lower case.",
        "builtin::string",
    ),
    item(
        "ToUpper",
        "func ToUpper(s string) string",
        "Returns s mapped to upper case.",
        "builtin::string",
    ),
    item(
        "Repeat",
        "func Repeat(s string, count int) string",
        "Returns count copies of s.",
        "builtin::string",
    ),
    item(
        "Index",
        "func Index(s, substr string) int",
        "Index of the first substr, or -1.",
        "builtin::int",
    ),
];

const STRINGS_BUILDER: &[Item] = &[
    item(
        "Write",
        "func (b *Builder) Write(p []byte) (int, error)",
        "Appends bytes.",
        "builtin::int",
    ),
    item(
        "WriteString",
        "func (b *Builder) WriteString(s string) (int, error)",
        "Appends a string.",
        "builtin::int",
    ),
    item(
        "String",
        "func (b *Builder) String() string",
        "Returns the accumulated string.",
        "builtin::string",
    ),
    item(
        "Len",
        "func (b *Builder) Len() int",
        "Number of accumulated bytes.",
        "builtin::int",
    ),
    item(
        "Reset",
        "func (b *Builder) Reset()",
        "Clears the builder.",
        "",
    ),
    item(
        "Grow",
        "func (b *Builder) Grow(n int)",
        "Grows the buffer.",
        "",
    ),
];

const STRCONV: &[Item] = &[
    item(
        "Atoi",
        "func Atoi(s string) (int, error)",
        "Parses an int.",
        "builtin::int",
    ),
    item(
        "Itoa",
        "func Itoa(i int) string",
        "Formats an int.",
        "builtin::string",
    ),
    item(
        "ParseInt",
        "func ParseInt(s string, base int, bitSize int) (int64, error)",
        "Parses an integer.",
        "",
    ),
    item(
        "ParseFloat",
        "func ParseFloat(s string, bitSize int) (float64, error)",
        "Parses a float.",
        "",
    ),
    item(
        "ParseBool",
        "func ParseBool(str string) (bool, error)",
        "Parses a boolean.",
        "builtin::bool",
    ),
    item(
        "FormatInt",
        "func FormatInt(i int64, base int) string",
        "Formats an integer.",
        "builtin::string",
    ),
    item(
        "FormatBool",
        "func FormatBool(b bool) string",
        "Formats a boolean.",
        "builtin::string",
    ),
    item(
        "Quote",
        "func Quote(s string) string",
        "Returns a double-quoted Go string literal.",
        "builtin::string",
    ),
];

const OS_FILE: &[Item] = &[
    item(
        "Read",
        "func (f *File) Read(b []byte) (n int, err error)",
        "Reads into b.",
        "builtin::int",
    ),
    item(
        "Write",
        "func (f *File) Write(b []byte) (n int, err error)",
        "Writes b.",
        "builtin::int",
    ),
    item(
        "Close",
        "func (f *File) Close() error",
        "Closes the file.",
        "builtin::error",
    ),
    item(
        "Stat",
        "func (f *File) Stat() (FileInfo, error)",
        "Returns file metadata.",
        "os::FileInfo",
    ),
    item(
        "Name",
        "func (f *File) Name() string",
        "Returns the file name.",
        "builtin::string",
    ),
    item(
        "Seek",
        "func (f *File) Seek(offset int64, whence int) (int64, error)",
        "Sets the offset.",
        "",
    ),
];

const OS: &[Item] = &[
    item(
        "Open",
        "func Open(name string) (*File, error)",
        "Opens a file for reading.",
        "os::File",
    ),
    item(
        "Create",
        "func Create(name string) (*File, error)",
        "Creates or truncates a file.",
        "os::File",
    ),
    item(
        "ReadFile",
        "func ReadFile(name string) ([]byte, error)",
        "Reads an entire file.",
        "",
    ),
    item(
        "WriteFile",
        "func WriteFile(name string, data []byte, perm FileMode) error",
        "Writes data to a file.",
        "builtin::error",
    ),
    item(
        "Getenv",
        "func Getenv(key string) string",
        "Returns an environment variable.",
        "builtin::string",
    ),
    item(
        "Setenv",
        "func Setenv(key, value string) error",
        "Sets an environment variable.",
        "builtin::error",
    ),
    item("Exit", "func Exit(code int)", "Terminates the process.", ""),
    item(
        "MkdirAll",
        "func MkdirAll(path string, perm FileMode) error",
        "Creates a directory and parents.",
        "builtin::error",
    ),
    item(
        "Remove",
        "func Remove(name string) error",
        "Removes a file or empty directory.",
        "builtin::error",
    ),
    item(
        "Stat",
        "func Stat(name string) (FileInfo, error)",
        "Returns file metadata.",
        "os::FileInfo",
    ),
    field(
        "Args",
        "var Args []string",
        "Command-line arguments, starting with the program name.",
        "",
    ),
    field("Stdout", "var Stdout *File", "Standard output.", "os::File"),
    field("Stderr", "var Stderr *File", "Standard error.", "os::File"),
    field("Stdin", "var Stdin *File", "Standard input.", "os::File"),
];

const IO: &[Item] = &[
    item(
        "ReadAll",
        "func ReadAll(r Reader) ([]byte, error)",
        "Reads until EOF.",
        "",
    ),
    item(
        "Copy",
        "func Copy(dst Writer, src Reader) (written int64, err error)",
        "Copies until EOF.",
        "",
    ),
    field(
        "EOF",
        "var EOF error",
        "The error returned by Read at the end of input.",
        "builtin::error",
    ),
];

const CONTEXT_TYPE: &[Item] = &[
    item(
        "Deadline",
        "func (c Context) Deadline() (deadline time.Time, ok bool)",
        "Returns the deadline.",
        "time::Time",
    ),
    item(
        "Done",
        "func (c Context) Done() <-chan struct{}",
        "Closes when the context is canceled.",
        "",
    ),
    item(
        "Err",
        "func (c Context) Err() error",
        "Returns the cancellation error.",
        "builtin::error",
    ),
    item(
        "Value",
        "func (c Context) Value(key any) any",
        "Returns the value stored for key.",
        "builtin::any",
    ),
];

const CONTEXT: &[Item] = &[
    item(
        "Background",
        "func Background() Context",
        "Returns a never-canceled context.",
        "context::Context",
    ),
    item(
        "TODO",
        "func TODO() Context",
        "Returns a placeholder context.",
        "context::Context",
    ),
    item(
        "WithCancel",
        "func WithCancel(parent Context) (ctx Context, cancel CancelFunc)",
        "Returns a cancelable child.",
        "context::Context",
    ),
    item(
        "WithTimeout",
        "func WithTimeout(parent Context, timeout time.Duration) (Context, CancelFunc)",
        "Cancels after a timeout.",
        "context::Context",
    ),
    item(
        "WithValue",
        "func WithValue(parent Context, key, val any) Context",
        "Returns a child carrying a value.",
        "context::Context",
    ),
];

const HTTP_REQUEST: &[Item] = &[
    field("Method", "Method string", "HTTP method.", "builtin::string"),
    field("URL", "URL *url.URL", "Request URL.", ""),
    field(
        "Header",
        "Header Header",
        "Request headers.",
        "net/http::Header",
    ),
    field("Body", "Body io.ReadCloser", "Request body.", ""),
    field(
        "Host",
        "Host string",
        "Host header value.",
        "builtin::string",
    ),
    item(
        "Context",
        "func (r *Request) Context() context.Context",
        "Returns the request context.",
        "context::Context",
    ),
    item(
        "Cookie",
        "func (r *Request) Cookie(name string) (*Cookie, error)",
        "Returns a named cookie.",
        "net/http::Cookie",
    ),
    item(
        "FormValue",
        "func (r *Request) FormValue(key string) string",
        "Returns the first form value.",
        "builtin::string",
    ),
    item(
        "PathValue",
        "func (r *Request) PathValue(name string) string",
        "Returns a path wildcard.",
        "builtin::string",
    ),
    item(
        "Clone",
        "func (r *Request) Clone(ctx context.Context) *Request",
        "Returns a deep copy using ctx.",
        "net/http::Request",
    ),
];

const HTTP_RESPONSE: &[Item] = &[
    field("Status", "Status string", "Status line.", "builtin::string"),
    field(
        "StatusCode",
        "StatusCode int",
        "Status code.",
        "builtin::int",
    ),
    field(
        "Header",
        "Header Header",
        "Response headers.",
        "net/http::Header",
    ),
    field("Body", "Body io.ReadCloser", "Response body.", ""),
];

const HTTP_CLIENT: &[Item] = &[
    item(
        "Do",
        "func (c *Client) Do(req *Request) (*Response, error)",
        "Sends a request.",
        "net/http::Response",
    ),
    item(
        "Get",
        "func (c *Client) Get(url string) (*Response, error)",
        "Sends a GET request.",
        "net/http::Response",
    ),
    item(
        "Post",
        "func (c *Client) Post(url, contentType string, body io.Reader) (*Response, error)",
        "Sends a POST request.",
        "net/http::Response",
    ),
    item(
        "Head",
        "func (c *Client) Head(url string) (*Response, error)",
        "Sends a HEAD request.",
        "net/http::Response",
    ),
    item(
        "CloseIdleConnections",
        "func (c *Client) CloseIdleConnections()",
        "Closes idle connections.",
        "",
    ),
];

const HTTP: &[Item] = &[
    item(
        "Get",
        "func Get(url string) (*Response, error)",
        "Sends a GET request.",
        "net/http::Response",
    ),
    item(
        "Post",
        "func Post(url, contentType string, body io.Reader) (*Response, error)",
        "Sends a POST request.",
        "net/http::Response",
    ),
    item(
        "Head",
        "func Head(url string) (*Response, error)",
        "Sends a HEAD request.",
        "net/http::Response",
    ),
    item(
        "NewRequest",
        "func NewRequest(method, url string, body io.Reader) (*Request, error)",
        "Builds a request.",
        "net/http::Request",
    ),
    item(
        "ListenAndServe",
        "func ListenAndServe(addr string, handler Handler) error",
        "Listens and serves HTTP.",
        "builtin::error",
    ),
    item(
        "HandleFunc",
        "func HandleFunc(pattern string, handler func(ResponseWriter, *Request))",
        "Registers a handler function.",
        "",
    ),
    item(
        "Handle",
        "func Handle(pattern string, handler Handler)",
        "Registers a handler.",
        "",
    ),
    item(
        "Error",
        "func Error(w ResponseWriter, error string, code int)",
        "Writes an HTTP error.",
        "",
    ),
    item(
        "Serve",
        "func Serve(l net.Listener, handler Handler) error",
        "Serves connections.",
        "builtin::error",
    ),
];

const JSON: &[Item] = &[
    item(
        "Marshal",
        "func Marshal(v any) ([]byte, error)",
        "Encodes v as JSON.",
        "",
    ),
    item(
        "Unmarshal",
        "func Unmarshal(data []byte, v any) error",
        "Parses JSON into v.",
        "builtin::error",
    ),
    item(
        "MarshalIndent",
        "func MarshalIndent(v any, prefix, indent string) ([]byte, error)",
        "Encodes indented JSON.",
        "",
    ),
    item(
        "NewEncoder",
        "func NewEncoder(w io.Writer) *Encoder",
        "Returns a streaming encoder.",
        "encoding/json::Encoder",
    ),
    item(
        "NewDecoder",
        "func NewDecoder(r io.Reader) *Decoder",
        "Returns a streaming decoder.",
        "encoding/json::Decoder",
    ),
    item(
        "Valid",
        "func Valid(data []byte) bool",
        "Reports whether data is valid JSON.",
        "builtin::bool",
    ),
];

const TIME_TIME: &[Item] = &[
    item(
        "Add",
        "func (t Time) Add(d Duration) Time",
        "Returns t+d.",
        "time::Time",
    ),
    item(
        "After",
        "func (t Time) After(u Time) bool",
        "Reports whether t is after u.",
        "builtin::bool",
    ),
    item(
        "Before",
        "func (t Time) Before(u Time) bool",
        "Reports whether t is before u.",
        "builtin::bool",
    ),
    item(
        "Equal",
        "func (t Time) Equal(u Time) bool",
        "Reports whether t and u are the same instant.",
        "builtin::bool",
    ),
    item(
        "Format",
        "func (t Time) Format(layout string) string",
        "Formats with a reference layout.",
        "builtin::string",
    ),
    item(
        "IsZero",
        "func (t Time) IsZero() bool",
        "Reports whether t is the zero time.",
        "builtin::bool",
    ),
    item(
        "Sub",
        "func (t Time) Sub(u Time) Duration",
        "Returns t-u.",
        "time::Duration",
    ),
    item(
        "Unix",
        "func (t Time) Unix() int64",
        "Returns seconds since the Unix epoch.",
        "",
    ),
    item(
        "UTC",
        "func (t Time) UTC() Time",
        "Returns t in UTC.",
        "time::Time",
    ),
    item(
        "String",
        "func (t Time) String() string",
        "Returns a textual time.",
        "builtin::string",
    ),
    item(
        "Year",
        "func (t Time) Year() int",
        "Returns the year.",
        "builtin::int",
    ),
    item(
        "Month",
        "func (t Time) Month() Month",
        "Returns the month.",
        "",
    ),
    item(
        "Day",
        "func (t Time) Day() int",
        "Returns the day of the month.",
        "builtin::int",
    ),
];

const TIME_DURATION: &[Item] = &[
    item(
        "Hours",
        "func (d Duration) Hours() float64",
        "Duration as hours.",
        "",
    ),
    item(
        "Minutes",
        "func (d Duration) Minutes() float64",
        "Duration as minutes.",
        "",
    ),
    item(
        "Seconds",
        "func (d Duration) Seconds() float64",
        "Duration as seconds.",
        "",
    ),
    item(
        "Milliseconds",
        "func (d Duration) Milliseconds() int64",
        "Duration as milliseconds.",
        "",
    ),
    item(
        "String",
        "func (d Duration) String() string",
        "Returns a textual duration.",
        "builtin::string",
    ),
    item(
        "Abs",
        "func (d Duration) Abs() Duration",
        "Returns the absolute duration.",
        "time::Duration",
    ),
];

const TIME: &[Item] = &[
    item(
        "Now",
        "func Now() Time",
        "Returns the current local time.",
        "time::Time",
    ),
    item(
        "Parse",
        "func Parse(layout, value string) (Time, error)",
        "Parses a formatted time.",
        "time::Time",
    ),
    item(
        "Since",
        "func Since(t Time) Duration",
        "Returns the time elapsed since t.",
        "time::Duration",
    ),
    item(
        "Until",
        "func Until(t Time) Duration",
        "Returns the duration until t.",
        "time::Duration",
    ),
    item(
        "Sleep",
        "func Sleep(d Duration)",
        "Pauses the current goroutine.",
        "",
    ),
    item(
        "NewTicker",
        "func NewTicker(d Duration) *Ticker",
        "Returns a ticker.",
        "time::Ticker",
    ),
    item(
        "NewTimer",
        "func NewTimer(d Duration) *Timer",
        "Returns a timer.",
        "time::Timer",
    ),
    field(
        "Second",
        "const Second Duration",
        "One second.",
        "time::Duration",
    ),
    field(
        "Minute",
        "const Minute Duration",
        "One minute.",
        "time::Duration",
    ),
    field("Hour", "const Hour Duration", "One hour.", "time::Duration"),
];

const SYNC_MUTEX: &[Item] = &[
    item("Lock", "func (m *Mutex) Lock()", "Locks the mutex.", ""),
    item(
        "Unlock",
        "func (m *Mutex) Unlock()",
        "Unlocks the mutex.",
        "",
    ),
    item(
        "TryLock",
        "func (m *Mutex) TryLock() bool",
        "Tries to lock.",
        "builtin::bool",
    ),
];

const SYNC_WAIT: &[Item] = &[
    item(
        "Add",
        "func (wg *WaitGroup) Add(delta int)",
        "Adds delta to the counter.",
        "",
    ),
    item(
        "Done",
        "func (wg *WaitGroup) Done()",
        "Decrements the counter.",
        "",
    ),
    item(
        "Wait",
        "func (wg *WaitGroup) Wait()",
        "Blocks until the counter is zero.",
        "",
    ),
];

const BYTES_BUFFER: &[Item] = &[
    item(
        "Write",
        "func (b *Buffer) Write(p []byte) (n int, err error)",
        "Appends bytes.",
        "builtin::int",
    ),
    item(
        "WriteString",
        "func (b *Buffer) WriteString(s string) (n int, err error)",
        "Appends a string.",
        "builtin::int",
    ),
    item(
        "String",
        "func (b *Buffer) String() string",
        "Returns the contents as a string.",
        "builtin::string",
    ),
    item(
        "Bytes",
        "func (b *Buffer) Bytes() []byte",
        "Returns the contents.",
        "",
    ),
    item(
        "Len",
        "func (b *Buffer) Len() int",
        "Number of bytes.",
        "builtin::int",
    ),
    item(
        "Reset",
        "func (b *Buffer) Reset()",
        "Clears the buffer.",
        "",
    ),
    item(
        "Read",
        "func (b *Buffer) Read(p []byte) (n int, err error)",
        "Reads from the buffer.",
        "builtin::int",
    ),
];

const BYTES: &[Item] = &[
    item(
        "Contains",
        "func Contains(b, subslice []byte) bool",
        "Reports whether subslice is in b.",
        "builtin::bool",
    ),
    item(
        "Equal",
        "func Equal(a, b []byte) bool",
        "Reports whether a and b are equal.",
        "builtin::bool",
    ),
    item(
        "Split",
        "func Split(s, sep []byte) [][]byte",
        "Splits around sep.",
        "",
    ),
    item(
        "TrimSpace",
        "func TrimSpace(s []byte) []byte",
        "Removes leading and trailing spaces.",
        "",
    ),
    item(
        "NewBuffer",
        "func NewBuffer(buf []byte) *Buffer",
        "Returns a buffer using buf.",
        "bytes::Buffer",
    ),
];

const LOG: &[Item] = &[
    item(
        "Print",
        "func Print(v ...any)",
        "Writes to the standard logger.",
        "",
    ),
    item(
        "Printf",
        "func Printf(format string, v ...any)",
        "Writes a formatted line.",
        "",
    ),
    item(
        "Println",
        "func Println(v ...any)",
        "Writes a spaced line.",
        "",
    ),
    item("Fatal", "func Fatal(v ...any)", "Writes and exits.", ""),
    item(
        "Fatalf",
        "func Fatalf(format string, v ...any)",
        "Writes a formatted line and exits.",
        "",
    ),
    item("Panic", "func Panic(v ...any)", "Writes and panics.", ""),
];

const SLICES: &[Item] = &[
    item(
        "Contains",
        "func Contains[S ~[]E, E comparable](s S, v E) bool",
        "Reports whether v is an element.",
        "builtin::bool",
    ),
    item(
        "Clone",
        "func Clone[S ~[]E, E any](s S) S",
        "Returns a copy of the slice.",
        "",
    ),
    item(
        "Equal",
        "func Equal[S ~[]E, E comparable](s1, s2 S) bool",
        "Reports whether the slices are equal.",
        "builtin::bool",
    ),
    item(
        "Sort",
        "func Sort[S ~[]E, E cmp.Ordered](x S)",
        "Sorts a slice in ascending order.",
        "",
    ),
    item(
        "Index",
        "func Index[S ~[]E, E comparable](s S, v E) int",
        "Index of the first v, or -1.",
        "builtin::int",
    ),
    item(
        "Delete",
        "func Delete[S ~[]E, E any](s S, i, j int) S",
        "Removes elements i..j.",
        "",
    ),
];

const MAPS: &[Item] = &[
    item(
        "Clone",
        "func Clone[M ~map[K]V, K comparable, V any](m M) M",
        "Returns a shallow copy.",
        "",
    ),
    item(
        "Keys",
        "func Keys[M ~map[K]V, K comparable, V any](m M) []K",
        "Returns the keys.",
        "",
    ),
    item(
        "Values",
        "func Values[M ~map[K]V, K comparable, V any](m M) []V",
        "Returns the values.",
        "",
    ),
    item(
        "Equal",
        "func Equal[M1, M2 ~map[K]V, K, V comparable](m1 M1, m2 M2) bool",
        "Reports whether the maps are equal.",
        "builtin::bool",
    ),
];

const FILEPATH: &[Item] = &[
    item(
        "Join",
        "func Join(elem ...string) string",
        "Joins path elements.",
        "builtin::string",
    ),
    item(
        "Base",
        "func Base(path string) string",
        "Returns the last element.",
        "builtin::string",
    ),
    item(
        "Dir",
        "func Dir(path string) string",
        "Returns all but the last element.",
        "builtin::string",
    ),
    item(
        "Ext",
        "func Ext(path string) string",
        "Returns the extension.",
        "builtin::string",
    ),
    item(
        "Clean",
        "func Clean(path string) string",
        "Returns the shortest equivalent path.",
        "builtin::string",
    ),
    item(
        "Abs",
        "func Abs(path string) (string, error)",
        "Returns an absolute path.",
        "builtin::string",
    ),
];

const REGEXP_TYPE: &[Item] = &[
    item(
        "Match",
        "func (re *Regexp) Match(b []byte) bool",
        "Reports whether b matches.",
        "builtin::bool",
    ),
    item(
        "MatchString",
        "func (re *Regexp) MatchString(s string) bool",
        "Reports whether s matches.",
        "builtin::bool",
    ),
    item(
        "FindString",
        "func (re *Regexp) FindString(s string) string",
        "Returns the first match.",
        "builtin::string",
    ),
    item(
        "FindAllString",
        "func (re *Regexp) FindAllString(s string, n int) []string",
        "Returns successive matches.",
        "",
    ),
    item(
        "ReplaceAllString",
        "func (re *Regexp) ReplaceAllString(src, repl string) string",
        "Replaces matches.",
        "builtin::string",
    ),
    item(
        "Split",
        "func (re *Regexp) Split(s string, n int) []string",
        "Splits around matches.",
        "",
    ),
];

const REGEXP: &[Item] = &[
    item(
        "Compile",
        "func Compile(expr string) (*Regexp, error)",
        "Parses a regular expression.",
        "regexp::Regexp",
    ),
    item(
        "MustCompile",
        "func MustCompile(str string) *Regexp",
        "Parses a regular expression or panics.",
        "regexp::Regexp",
    ),
    item(
        "MatchString",
        "func MatchString(pattern string, s string) (bool, error)",
        "Reports whether s matches pattern.",
        "builtin::bool",
    ),
];

const BUFIO_READER: &[Item] = &[
    item(
        "Read",
        "func (b *Reader) Read(p []byte) (n int, err error)",
        "Reads into p.",
        "builtin::int",
    ),
    item(
        "ReadString",
        "func (b *Reader) ReadString(delim byte) (string, error)",
        "Reads until delim.",
        "builtin::string",
    ),
    item(
        "ReadBytes",
        "func (b *Reader) ReadBytes(delim byte) ([]byte, error)",
        "Reads until delim.",
        "",
    ),
];

const BUFIO: &[Item] = &[
    item(
        "NewReader",
        "func NewReader(rd io.Reader) *Reader",
        "Returns a buffered reader.",
        "bufio::Reader",
    ),
    item(
        "NewWriter",
        "func NewWriter(w io.Writer) *Writer",
        "Returns a buffered writer.",
        "bufio::Writer",
    ),
    item(
        "NewScanner",
        "func NewScanner(r io.Reader) *Scanner",
        "Returns a scanner.",
        "bufio::Scanner",
    ),
];

const BUFIO_SCANNER: &[Item] = &[
    item(
        "Scan",
        "func (s *Scanner) Scan() bool",
        "Advances to the next token.",
        "builtin::bool",
    ),
    item(
        "Text",
        "func (s *Scanner) Text() string",
        "Returns the current token.",
        "builtin::string",
    ),
    item(
        "Bytes",
        "func (s *Scanner) Bytes() []byte",
        "Returns the current token bytes.",
        "",
    ),
    item(
        "Err",
        "func (s *Scanner) Err() error",
        "Returns the first non-EOF error.",
        "builtin::error",
    ),
];

const TESTING_T: &[Item] = &[
    item(
        "Error",
        "func (c *T) Error(args ...any)",
        "Marks the test failed and continues.",
        "",
    ),
    item(
        "Errorf",
        "func (c *T) Errorf(format string, args ...any)",
        "Marks the test failed with a format.",
        "",
    ),
    item(
        "Fatal",
        "func (c *T) Fatal(args ...any)",
        "Marks the test failed and stops it.",
        "",
    ),
    item(
        "Fatalf",
        "func (c *T) Fatalf(format string, args ...any)",
        "Marks the test failed and stops it.",
        "",
    ),
    item(
        "Log",
        "func (c *T) Log(args ...any)",
        "Records a log line.",
        "",
    ),
    item(
        "Logf",
        "func (c *T) Logf(format string, args ...any)",
        "Records a formatted log line.",
        "",
    ),
    item(
        "Run",
        "func (t *T) Run(name string, f func(t *T)) bool",
        "Runs a subtest.",
        "builtin::bool",
    ),
    item(
        "Helper",
        "func (c *T) Helper()",
        "Marks the caller as a helper.",
        "",
    ),
    item(
        "Skip",
        "func (c *T) Skip(args ...any)",
        "Skips the test.",
        "",
    ),
    item(
        "Skipf",
        "func (c *T) Skipf(format string, args ...any)",
        "Skips the test with a format.",
        "",
    ),
    item(
        "Parallel",
        "func (t *T) Parallel()",
        "Marks the test parallel.",
        "",
    ),
    item(
        "Cleanup",
        "func (c *T) Cleanup(f func())",
        "Registers a cleanup function.",
        "",
    ),
    item(
        "Failed",
        "func (c *T) Failed() bool",
        "Reports whether the test failed.",
        "builtin::bool",
    ),
    item(
        "Name",
        "func (c *T) Name() string",
        "Returns the test name.",
        "builtin::string",
    ),
];

const SORT: &[Item] = &[
    item(
        "Strings",
        "func Strings(x []string)",
        "Sorts a slice of strings.",
        "",
    ),
    item("Ints", "func Ints(x []int)", "Sorts a slice of ints.", ""),
    item(
        "Slice",
        "func Slice(x any, less func(i, j int) bool)",
        "Sorts a slice with a comparison.",
        "",
    ),
    item(
        "Search",
        "func Search(n int, f func(int) bool) int",
        "Binary search.",
        "builtin::int",
    ),
];

const MATH: &[Item] = &[
    item("Abs", "func Abs(x float64) float64", "Absolute value.", ""),
    item("Sqrt", "func Sqrt(x float64) float64", "Square root.", ""),
    item("Ceil", "func Ceil(x float64) float64", "Round up.", ""),
    item("Floor", "func Floor(x float64) float64", "Round down.", ""),
    item(
        "Max",
        "func Max(x, y float64) float64",
        "Larger of two numbers.",
        "",
    ),
    item(
        "Min",
        "func Min(x, y float64) float64",
        "Smaller of two numbers.",
        "",
    ),
    item(
        "Round",
        "func Round(x float64) float64",
        "Round to nearest.",
        "",
    ),
    item("Pow", "func Pow(x, y float64) float64", "x**y.", ""),
];

/// Packages offered by name and after their default alias.
pub const PACKAGES: &[Package] = &[
    Package {
        path: "fmt",
        alias: "fmt",
        documentation: "Formatted I/O.",
        items: FMT,
        types: &[],
    },
    Package {
        path: "errors",
        alias: "errors",
        documentation: "Error helpers.",
        items: ERRORS,
        types: &[],
    },
    Package {
        path: "strings",
        alias: "strings",
        documentation: "UTF-8 string helpers.",
        items: STRINGS,
        types: &[Type {
            name: "Builder",
            documentation: "A string builder.",
            members: STRINGS_BUILDER,
        }],
    },
    Package {
        path: "strconv",
        alias: "strconv",
        documentation: "Conversions to and from strings.",
        items: STRCONV,
        types: &[],
    },
    Package {
        path: "os",
        alias: "os",
        documentation: "Operating system files and environment.",
        items: OS,
        types: &[Type {
            name: "File",
            documentation: "An open file descriptor.",
            members: OS_FILE,
        }],
    },
    Package {
        path: "io",
        alias: "io",
        documentation: "Basic I/O interfaces.",
        items: IO,
        types: &[],
    },
    Package {
        path: "context",
        alias: "context",
        documentation: "Cancellation and request-scoped values.",
        items: CONTEXT,
        types: &[Type {
            name: "Context",
            documentation: "A cancellation and value carrier.",
            members: CONTEXT_TYPE,
        }],
    },
    Package {
        path: "net/http",
        alias: "http",
        documentation: "HTTP client and server.",
        items: HTTP,
        types: &[
            Type {
                name: "Request",
                documentation: "An HTTP request.",
                members: HTTP_REQUEST,
            },
            Type {
                name: "Response",
                documentation: "An HTTP response.",
                members: HTTP_RESPONSE,
            },
            Type {
                name: "Client",
                documentation: "An HTTP client.",
                members: HTTP_CLIENT,
            },
        ],
    },
    Package {
        path: "encoding/json",
        alias: "json",
        documentation: "JSON encoding and decoding.",
        items: JSON,
        types: &[
            Type {
                name: "Encoder",
                documentation: "A streaming JSON encoder.",
                members: &[item(
                    "Encode",
                    "func (enc *Encoder) Encode(v any) error",
                    "Writes one JSON value.",
                    "builtin::error",
                )],
            },
            Type {
                name: "Decoder",
                documentation: "A streaming JSON decoder.",
                members: &[item(
                    "Decode",
                    "func (dec *Decoder) Decode(v any) error",
                    "Reads the next JSON value.",
                    "builtin::error",
                )],
            },
        ],
    },
    Package {
        path: "time",
        alias: "time",
        documentation: "Times and durations.",
        items: TIME,
        types: &[
            Type {
                name: "Time",
                documentation: "An instant in time.",
                members: TIME_TIME,
            },
            Type {
                name: "Duration",
                documentation: "Elapsed time.",
                members: TIME_DURATION,
            },
        ],
    },
    Package {
        path: "sync",
        alias: "sync",
        documentation: "Synchronization primitives.",
        items: &[],
        types: &[
            Type {
                name: "Mutex",
                documentation: "A mutual exclusion lock.",
                members: SYNC_MUTEX,
            },
            Type {
                name: "WaitGroup",
                documentation: "Waits for a collection of goroutines.",
                members: SYNC_WAIT,
            },
            Type {
                name: "Once",
                documentation: "Runs an action once.",
                members: &[item(
                    "Do",
                    "func (o *Once) Do(f func())",
                    "Calls f if it has not been called.",
                    "",
                )],
            },
        ],
    },
    Package {
        path: "bytes",
        alias: "bytes",
        documentation: "Byte slice helpers.",
        items: BYTES,
        types: &[Type {
            name: "Buffer",
            documentation: "A variable-sized byte buffer.",
            members: BYTES_BUFFER,
        }],
    },
    Package {
        path: "log",
        alias: "log",
        documentation: "Simple logging.",
        items: LOG,
        types: &[],
    },
    Package {
        path: "slices",
        alias: "slices",
        documentation: "Generic slice helpers.",
        items: SLICES,
        types: &[],
    },
    Package {
        path: "maps",
        alias: "maps",
        documentation: "Generic map helpers.",
        items: MAPS,
        types: &[],
    },
    Package {
        path: "path/filepath",
        alias: "filepath",
        documentation: "Operating-system file paths.",
        items: FILEPATH,
        types: &[],
    },
    Package {
        path: "path",
        alias: "path",
        documentation: "Slash-separated paths.",
        items: &[
            item(
                "Join",
                "func Join(elem ...string) string",
                "Joins path elements.",
                "builtin::string",
            ),
            item(
                "Base",
                "func Base(path string) string",
                "Returns the last element.",
                "builtin::string",
            ),
            item(
                "Dir",
                "func Dir(path string) string",
                "Returns all but the last element.",
                "builtin::string",
            ),
            item(
                "Ext",
                "func Ext(path string) string",
                "Returns the extension.",
                "builtin::string",
            ),
        ],
        types: &[],
    },
    Package {
        path: "regexp",
        alias: "regexp",
        documentation: "Regular expressions.",
        items: REGEXP,
        types: &[Type {
            name: "Regexp",
            documentation: "A compiled regular expression.",
            members: REGEXP_TYPE,
        }],
    },
    Package {
        path: "bufio",
        alias: "bufio",
        documentation: "Buffered I/O.",
        items: BUFIO,
        types: &[
            Type {
                name: "Reader",
                documentation: "A buffered reader.",
                members: BUFIO_READER,
            },
            Type {
                name: "Scanner",
                documentation: "A token scanner.",
                members: BUFIO_SCANNER,
            },
        ],
    },
    Package {
        path: "testing",
        alias: "testing",
        documentation: "Automated tests.",
        items: &[],
        types: &[Type {
            name: "T",
            documentation: "A single test.",
            members: TESTING_T,
        }],
    },
    Package {
        path: "sort",
        alias: "sort",
        documentation: "Sorting.",
        items: SORT,
        types: &[],
    },
    Package {
        path: "math",
        alias: "math",
        documentation: "Basic mathematical functions.",
        items: MATH,
        types: &[],
    },
];

/// Predeclared type names.
pub const PREDECLARED: &[&str] = &[
    "any",
    "bool",
    "byte",
    "comparable",
    "complex64",
    "complex128",
    "error",
    "float32",
    "float64",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "rune",
    "string",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
];

/// Members of a predeclared type. Only `error` has a method.
#[must_use]
pub fn builtin_type(name: &str) -> Option<&'static [Item]> {
    (name == "error").then_some(ERROR)
}

/// A package by import path or default alias.
#[must_use]
pub fn package(name: &str) -> Option<&'static Package> {
    PACKAGES
        .iter()
        .find(|package| package.path == name || package.alias == name)
}

/// A type `path::Name` from the catalog.
#[must_use]
pub fn catalog_type(fqn: &str) -> Option<(&'static Package, &'static Type)> {
    let (path, name) = fqn.rsplit_once("::")?;
    let package = package(path)?;
    package
        .types
        .iter()
        .find(|kind| kind.name == name)
        .map(|kind| (package, kind))
}

/// Result types of a catalog function. The first result is the useful receiver type.
#[must_use]
pub fn func_return(path: &str, name: &str) -> Option<&'static str> {
    let package = package(path)?;
    package
        .items
        .iter()
        .find(|item| item.name == name)
        .map(|item| item.returns)
        .filter(|returns| !returns.is_empty())
}

/// Result of a predeclared function when it is a known named type.
#[must_use]
pub fn builtin_return(name: &str) -> Option<&'static str> {
    BUILTINS
        .iter()
        .find(|item| item.name == name)
        .map(|item| item.returns)
        .filter(|returns| !returns.is_empty())
}

/// Markdown hover for a signature.
#[must_use]
pub fn hover_markup(signature: &str, documentation: &str) -> Hover {
    let mut value = format!("```go\n{signature}\n```");
    if !documentation.is_empty() {
        value.push_str("\n\n");
        value.push_str(documentation);
    }
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: None,
    }
}

/// Exact built-in and standard-library documentation.
pub struct GoDocs;

impl LanguageDocProvider for GoDocs {
    fn resolve_symbol_info(&self, language_id: &str, symbol: &str) -> Option<SymbolDocInfo> {
        (language_id == "go")
            .then(|| Self::lookup(symbol))
            .flatten()
    }

    fn resolve_doc_url(
        &self,
        language_id: &str,
        symbol: &str,
        _entity_type: EntityKind,
    ) -> Option<String> {
        (language_id == "go")
            .then(|| Self::lookup(symbol))
            .flatten()
            .and_then(|info| match info.target? {
                DefinitionTarget::ExternalUrl(url) => Some(url),
                DefinitionTarget::LocalFile { .. } => None,
            })
    }
}

impl GoDocs {
    fn lookup(symbol: &str) -> Option<SymbolDocInfo> {
        if let Some(item) = BUILTINS
            .iter()
            .chain(CONSTANTS)
            .find(|item| item.name == symbol)
        {
            return Some(info(
                symbol,
                EntityKind::Function,
                item.signature,
                item.documentation,
                &format!("https://pkg.go.dev/builtin#{symbol}"),
            ));
        }
        if let Some((package_name, name)) = symbol.rsplit_once('.') {
            let package = package(package_name)?;
            if let Some(item) = package.items.iter().find(|item| item.name == name) {
                return Some(info(
                    symbol,
                    if item.field {
                        EntityKind::Constant
                    } else {
                        EntityKind::Function
                    },
                    item.signature,
                    item.documentation,
                    &format!("https://pkg.go.dev/{}#{name}", package.path),
                ));
            }
            if let Some(kind) = package.types.iter().find(|kind| kind.name == name) {
                return Some(info(
                    symbol,
                    EntityKind::Class,
                    &format!("type {name}"),
                    kind.documentation,
                    &format!("https://pkg.go.dev/{}#{name}", package.path),
                ));
            }
        }
        None
    }
}

fn info(
    name: &str,
    kind: EntityKind,
    signature: &str,
    description: &str,
    url: &str,
) -> SymbolDocInfo {
    SymbolDocInfo {
        name: name.to_string(),
        kind,
        signature: signature.to_string(),
        description: description.to_string(),
        target: Some(DefinitionTarget::ExternalUrl(url.to_string())),
    }
}
