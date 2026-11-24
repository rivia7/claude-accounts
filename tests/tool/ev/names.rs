//! Python 各种类型真有的属性名（3.14 的 dir()，不含下划线开头的）。
//! 用处只有一个：碰到这里没实现的名字时分清两种情况——Python 有这个方法、只是这里没写（退出码 2），
//! 还是 Python 也没有（AttributeError，退出码 1）。都是编译时就定好的表，启动时不做任何事。

#[rustfmt::skip]
pub const STR: &[&str] = &[
    "capitalize", "casefold", "center", "count", "encode", "endswith", "expandtabs", "find", "format", "format_map", "index",
    "isalnum", "isalpha", "isascii", "isdecimal", "isdigit", "isidentifier", "islower", "isnumeric", "isprintable", "isspace",
    "istitle", "isupper", "join", "ljust", "lower", "lstrip", "maketrans", "partition", "removeprefix", "removesuffix",
    "replace", "rfind", "rindex", "rjust", "rpartition", "rsplit", "rstrip", "split", "splitlines", "startswith", "strip",
    "swapcase", "title", "translate", "upper", "zfill",
];

#[rustfmt::skip]
pub const LIST: &[&str] = &[
    "append", "clear", "copy", "count", "extend", "index", "insert", "pop", "remove", "reverse", "sort",
];

#[rustfmt::skip]
pub const DICT: &[&str] = &[
    "clear", "copy", "fromkeys", "get", "items", "keys", "pop", "popitem", "setdefault", "update", "values",
];

#[rustfmt::skip]
pub const SET: &[&str] = &[
    "add", "clear", "copy", "difference", "difference_update", "discard", "intersection", "intersection_update", "isdisjoint",
    "issubset", "issuperset", "pop", "remove", "symmetric_difference", "symmetric_difference_update", "union", "update",
];

#[rustfmt::skip]
pub const FROZENSET: &[&str] = &[
    "copy", "difference", "intersection", "isdisjoint", "issubset", "issuperset", "symmetric_difference", "union",
];

#[rustfmt::skip]
pub const TUPLE: &[&str] = &[
    "count", "index",
];

/// bool 也是这一份
#[rustfmt::skip]
pub const INT: &[&str] = &[
    "as_integer_ratio", "bit_count", "bit_length", "conjugate", "denominator", "from_bytes", "imag", "is_integer",
    "numerator", "real", "to_bytes",
];

#[rustfmt::skip]
pub const FLOAT: &[&str] = &[
    "as_integer_ratio", "conjugate", "from_number", "fromhex", "hex", "imag", "is_integer", "real",
];

#[rustfmt::skip]
pub const RANGE: &[&str] = &[
    "count", "index", "start", "step", "stop",
];

#[rustfmt::skip]
pub const TYPE: &[&str] = &[
    "mro",
];

/// dict_items 也是这一份
#[rustfmt::skip]
pub const DICT_KEYS: &[&str] = &[
    "isdisjoint", "mapping",
];

#[rustfmt::skip]
pub const DICT_VALUES: &[&str] = &[
    "mapping",
];

#[rustfmt::skip]
pub const GENERATOR: &[&str] = &[
    "close", "gi_code", "gi_frame", "gi_running", "gi_suspended", "gi_yieldfrom", "send", "throw",
];

#[rustfmt::skip]
pub const FILE: &[&str] = &[
    "buffer", "close", "closed", "detach", "encoding", "errors", "fileno", "flush", "isatty", "line_buffering", "mode", "name",
    "newlines", "read", "readable", "readline", "readlines", "reconfigure", "seek", "seekable", "tell", "truncate",
    "writable", "write", "write_through", "writelines",
];

#[rustfmt::skip]
pub const MOD_JSON: &[&str] = &[
    "JSONDecodeError", "JSONDecoder", "JSONEncoder", "codecs", "decoder", "detect_encoding", "dump", "dumps", "encoder",
    "load", "loads", "scanner",
];

#[rustfmt::skip]
pub const MOD_SYS: &[&str] = &[
    "abiflags", "activate_stack_trampoline", "addaudithook", "api_version", "argv", "audit", "base_exec_prefix",
    "base_prefix", "breakpointhook", "builtin_module_names", "byteorder", "call_tracing", "copyright",
    "deactivate_stack_trampoline", "displayhook", "dont_write_bytecode", "exc_info", "excepthook", "exception", "exec_prefix",
    "executable", "exit", "flags", "float_info", "float_repr_style", "get_asyncgen_hooks",
    "get_coroutine_origin_tracking_depth", "get_int_max_str_digits", "getallocatedblocks", "getdefaultencoding",
    "getdlopenflags", "getfilesystemencodeerrors", "getfilesystemencoding", "getprofile", "getrecursionlimit", "getrefcount",
    "getsizeof", "getswitchinterval", "gettrace", "getunicodeinternedsize", "hash_info", "hexversion", "implementation",
    "int_info", "intern", "is_finalizing", "is_remote_debug_enabled", "is_stack_trampoline_active", "maxsize", "maxunicode",
    "meta_path", "modules", "monitoring", "orig_argv", "path", "path_hooks", "path_importer_cache", "platform", "platlibdir",
    "prefix", "pycache_prefix", "remote_exec", "set_asyncgen_hooks", "set_coroutine_origin_tracking_depth",
    "set_int_max_str_digits", "setdlopenflags", "setprofile", "setrecursionlimit", "setswitchinterval", "settrace", "stderr",
    "stdin", "stdlib_module_names", "stdout", "thread_info", "unraisablehook", "version", "version_info", "warnoptions",
];

#[rustfmt::skip]
pub const MOD_TIME: &[&str] = &[
    "CLOCK_MONOTONIC", "CLOCK_MONOTONIC_RAW", "CLOCK_MONOTONIC_RAW_APPROX", "CLOCK_PROCESS_CPUTIME_ID", "CLOCK_REALTIME",
    "CLOCK_THREAD_CPUTIME_ID", "CLOCK_UPTIME_RAW", "CLOCK_UPTIME_RAW_APPROX", "altzone", "asctime", "clock_getres",
    "clock_gettime", "clock_gettime_ns", "clock_settime", "clock_settime_ns", "ctime", "daylight", "get_clock_info", "gmtime",
    "localtime", "mktime", "monotonic", "monotonic_ns", "perf_counter", "perf_counter_ns", "process_time", "process_time_ns",
    "sleep", "strftime", "strptime", "struct_time", "thread_time", "thread_time_ns", "time", "time_ns", "timezone", "tzname",
    "tzset",
];

/// Python 的内建名字（函数、类型、异常、常量）：没实现的说“没实现”，别的名字当成没绑上
#[rustfmt::skip]
pub const BUILTINS: &[&str] = &[
    "ArithmeticError", "AssertionError", "AttributeError", "BaseException", "BaseExceptionGroup", "BlockingIOError",
    "BrokenPipeError", "BufferError", "BytesWarning", "ChildProcessError", "ConnectionAbortedError", "ConnectionError",
    "ConnectionRefusedError", "ConnectionResetError", "DeprecationWarning", "EOFError", "Ellipsis", "EncodingWarning",
    "EnvironmentError", "Exception", "ExceptionGroup", "False", "FileExistsError", "FileNotFoundError", "FloatingPointError",
    "FutureWarning", "GeneratorExit", "IOError", "ImportError", "ImportWarning", "IndentationError", "IndexError",
    "InterruptedError", "IsADirectoryError", "KeyError", "KeyboardInterrupt", "LookupError", "MemoryError",
    "ModuleNotFoundError", "NameError", "None", "NotADirectoryError", "NotImplemented", "NotImplementedError", "OSError",
    "OverflowError", "PendingDeprecationWarning", "PermissionError", "ProcessLookupError", "PythonFinalizationError",
    "RecursionError", "ReferenceError", "ResourceWarning", "RuntimeError", "RuntimeWarning", "StopAsyncIteration",
    "StopIteration", "SyntaxError", "SyntaxWarning", "SystemError", "SystemExit", "TabError", "TimeoutError", "True",
    "TypeError", "UnboundLocalError", "UnicodeDecodeError", "UnicodeEncodeError", "UnicodeError", "UnicodeTranslateError",
    "UnicodeWarning", "UserWarning", "ValueError", "Warning", "ZeroDivisionError", "__import__", "abs", "aiter", "all",
    "anext", "any", "ascii", "bin", "bool", "breakpoint", "bytearray", "bytes", "callable", "chr", "classmethod", "compile",
    "complex", "copyright", "credits", "delattr", "dict", "dir", "divmod", "enumerate", "eval", "exec", "exit", "filter",
    "float", "format", "frozenset", "getattr", "globals", "hasattr", "hash", "help", "hex", "id", "input", "int",
    "isinstance", "issubclass", "iter", "len", "license", "list", "locals", "map", "max", "memoryview", "min", "next",
    "object", "oct", "open", "ord", "pow", "print", "property", "quit", "range", "repr", "reversed", "round", "set",
    "setattr", "slice", "sorted", "staticmethod", "str", "sum", "super", "tuple", "type", "vars", "zip",
];
