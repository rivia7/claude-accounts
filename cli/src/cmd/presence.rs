//! 设备统计（`presence_problems` 到 `cmd_presence_stop`、`past_time` 到 `cmd_presence_note`）：presence-agent、presence-wake、
//! presence-stop、presence-view、presence-note。
//!
//! 移植了：presence-wake、presence-stop，和别处要用的 `presence_problems`、`presence_load`、`spawn_background`。
//! 还没移植的：presence-agent（上报进程）、presence-view、presence-note。

use super::session::{cli_alive, proc_starts, ps_lstart};
use super::{arity, Cmd};
use crate::core::base::{CCA_HOME, CFG, SELF_PATH};
use crate::core::model::{is_api, parse_key};
use crate::core::policy::{policy_raw, POLICY_FILE};
use crate::core::tool::t;
use crate::dict;
use crate::py::base64::{b64decode, b64encode};
use crate::py::fs::{self, load_quiet, read_text};
use crate::py::hash::sha256;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::proc::{environ, Cmd as Proc, In, Out};
use crate::py::re::Re;
use crate::py::text;
use crate::py::time::sleep;
use crate::py::R;
use std::sync::LazyLock;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] =
    &[Cmd::all("presence-wake", cmd_presence_wake), Cmd::all("presence-stop", cmd_presence_stop), Cmd::all("presence-note", cmd_presence_note)];

pub static PRESENCE_DIR: LazyLock<String> = LazyLock::new(|| path::join(&CCA_HOME, "presence"));

pub static PRESENCE_URL_RE: LazyLock<Re> = LazyLock::new(|| {
    Re::new(concat!(
        r"^https://(?P<host>[0-9]{1,3}(\.[0-9]{1,3}){3}|\[[0-9A-Fa-f:]{2,39}\]|[A-Za-z0-9]([A-Za-z0-9.-]{0,251}[A-Za-z0-9])?)",
        r":(?P<port>[0-9]{1,5})/?$"
    ))
});
pub static PRESENCE_PIN_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^sha256//[A-Za-z0-9+/]{43}=$"));
pub static PRESENCE_SALT_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^([0-9a-f]{2}){16,64}$"));

/// 映射里 presence 一节的问题：{"url": "https://地址:端口", "pins": ["sha256//…"], "salt": "十六进制"}。维护者发布前
/// 严格检查；客户端只在完全合格时启用（不合格就当没有，映射的其余部分照用）
pub fn presence_problems(p: &V) -> Vec<String> {
    let Some(p) = p.as_dict() else {
        return vec!["presence 应该是 {\"url\": \"https://地址:端口\", \"pins\": [\"sha256//…\"], \"salt\": \"…\"} 形式的对象".to_string()];
    };
    let mut errs = Vec::new();
    let url = p.get("url");
    let port = url.as_ref().and_then(|u| u.as_str()).and_then(|u| PRESENCE_URL_RE.match_(u)).and_then(|m| m.name("port")).and_then(text::to_int);
    if !port.is_some_and(|n| (1..=65535).contains(&n)) {
        errs.push("presence.url 应该是 https://地址:端口（例如 https://203.0.113.10:18443）".to_string());
    }
    let pins_ok = p.list("pins").is_some_and(|pins| {
        (1..=4).contains(&pins.len()) && pins.items().iter().all(|x| x.as_str().is_some_and(|x| PRESENCE_PIN_RE.matches(x)))
    });
    if !pins_ok {
        errs.push("presence.pins 应该是 1～4 个服务器证书公钥的指纹（sha256//…，./policy.sh presence deploy 打印的）".to_string());
    }
    let salt = p.get("salt");
    if !salt.as_ref().and_then(|s| s.as_str()).is_some_and(|s| PRESENCE_SALT_RE.matches(s)) {
        errs.push("presence.salt 应该是 32～128 位（偶数位）小写十六进制".to_string());
    }
    errs
}

/// 公钥（base64 那一段）的指纹，和 ssh-keygen -l 的写法一样（SHA256:…）；不是 base64 是 None
pub fn ssh_fp(b64: &str) -> Option<String> {
    let raw = b64decode(b64, true).ok()?;
    Some(format!("SHA256:{}", b64encode(&sha256(&raw)).trim_end_matches('=')))
}

/// 这台 Mac 拉映射用的 deploy key：(私钥路径, 指纹)；没有、不是 ed25519 是 None。公钥文件不是 UTF-8 是 ValueError（调用方接）
pub fn presence_key() -> R<Option<(String, String)>> {
    let key = path::join_all(&[&CCA_HOME, "policy", "id_ed25519"]);
    let content = match read_text(&format!("{key}.pub")) {
        Ok(content) => content,
        Err(e) if e.is_os() => return Ok(None),
        Err(e) => return Err(e),
    };
    let fields = text::split_ws(&content);
    let fp = if fields.len() >= 2 && fields[0] == "ssh-ed25519" && path::isfile(&key) { ssh_fp(fields[1]) } else { None };
    Ok(fp.map(|fp| (key, fp)))
}

/// (用不了的原因, 配置)：能用时原因是空的，配置是 {url, pins, salt, key, fp}
pub fn presence_load() -> R<(String, Option<Dict>)> {
    let off = load_quiet(&CFG).and_then(|c| c.as_dict()).is_some_and(|c| matches!(c.get("presence"), Some(V::Bool(false))));
    if off {
        return Ok(("这台 Mac 关掉了设备统计（config.json 里写着 \"presence\": false）".into(), None));
    }
    let Some(j) = load_quiet(&POLICY_FILE).and_then(|j| j.as_dict()) else {
        return Ok(("这台 Mac 没有配置维护者的映射（cca policy setup）".into(), None));
    };
    let Some(p) = j.get("presence").filter(|p| !p.is_none()) else {
        return Ok(("维护者的映射里还没有设备统计的服务器".into(), None));
    };
    let errs = presence_problems(&p);
    if !errs.is_empty() {
        return Ok((format!("维护者的映射里设备统计那一节不对：{}", errs.join("；")), None));
    }
    let Some((key, fp)) = presence_key()? else {
        return Ok(("这台 Mac 没有映射的 deploy key（cca policy setup）".into(), None));
    };
    let p = p.as_dict().expect("presence_problems 核对过是字典");
    let cfg = dict! { "url" => p.at("url").py_str().trim_end_matches('/'), "pins" => p.at("pins"), "salt" => p.at("salt"), "key" => key, "fp" => fp };
    Ok((String::new(), Some(cfg)))
}

/// 上报用的账号摘要：服务器只看到它，看不到邮箱（盐在签名的映射里，各台 Mac 算出来的一样）
pub fn presence_aid(salt: &str, tool: &str, email: &str) -> R<String> {
    let compact: String = salt.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let key = crate::py::hash::from_hex(&compact).ok_or_else(|| crate::py::Fail::Value("non-hexadecimal number found in fromhex() arg".into()))?;
    let digest = crate::py::hash::hmac_sha256(&key, format!("{}\0{}", tool, email.to_lowercase()).as_bytes());
    Ok(crate::py::hash::to_hex(&digest)[..32].to_string())
}

/// 这个账号是映射里的官方账号吗（pm 是这个工具的 policy_raw）：只有它们上报、看得到设备
pub fn presence_mapped(pm: &Dict, key: &str, v: &V) -> bool {
    let e = parse_key(key).1.to_lowercase();
    !e.is_empty() && !is_api(v) && pm.has(&e)
}

/// 后台进程不带的：App 的提问会话、事件这些
pub const BG_ENV_DROP: [&str; 7] = ["CCA_EVENTS", "CCA_GRANT", "CCA_RO", "CCA_PROMPT", "CCA_TTY", "CCA_YES", "CCA_TOOL"];

/// 在后台起 cca 的一个子命令（上报进程、同步 API key、统计折合金额）：后台优先级（只用能效核，定时器和别的唤醒合在一起）、
/// 自己一个会话、不接终端、不带 BG_ENV_DROP。起不了就算了（静默）
pub fn spawn_background(args: &[&str]) {
    const TASKPOLICY: &str = "/usr/sbin/taskpolicy";
    // SAFETY: 路径是以 0 结尾的常量
    let has_taskpolicy = unsafe { libc::access(c"/usr/sbin/taskpolicy".as_ptr(), libc::X_OK) == 0 };
    let mut argv: Vec<&str> = if has_taskpolicy { vec![TASKPOLICY, "-c", "background"] } else { Vec::new() };
    argv.push(&SELF_PATH);
    argv.extend(args);
    let env: Vec<(String, String)> = environ().into_iter().filter(|(k, _)| !BG_ENV_DROP.contains(&k.as_str())).collect();
    let _ = Proc::new(argv).stdin(In::Null).stdout(Out::Null).stderr(Out::Null).env(env).new_session().spawn();
}

/// key（当前工具的）是映射里的官方账号、正在用：确保这台 Mac 的上报进程在跑（在跑的会经 kqueue 自己看到新的登记），
/// 不在就在后台起一个。静默：出什么错都不影响启动
pub fn presence_wake(key: &str) {
    let wake = || -> R<()> {
        if !presence_mapped(&policy_raw(t()), key, &V::None) || presence_load()?.1.is_none() {
            return Ok(());
        }
        fs::makedirs_mode(&PRESENCE_DIR, 0o700)?;
        {
            use std::os::unix::io::AsRawFd;
            let lock = std::fs::OpenOptions::new().append(true).create(true).open(path::join(&PRESENCE_DIR, "agent.lock"))?;
            // SAFETY: lock 开着，fd 有效
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Ok(()); // 在跑
            }
            // SAFETY: 同上
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) };
        }
        spawn_background(&["_presence-agent"]);
        Ok(())
    };
    match wake() {
        Err(e) if !e.is_os_or_value() => panic!("presence_wake：{e}"),
        _ => {}
    }
}

pub fn cmd_presence_wake(args: &[String]) -> R<()> {
    arity("cmd_presence_wake", args, 1, 1)?;
    presence_wake(&args[0]);
    Ok(())
}

/// cca policy 里的一句：这台 Mac 会不会把映射里账号的使用情况报告给维护者的服务器
pub fn cmd_presence_note(args: &[String]) -> R<()> {
    arity("cmd_presence_note", args, 0, 0)?;
    if !load_quiet(&POLICY_FILE).and_then(|j| j.as_dict()).is_some_and(|j| j.has("presence")) {
        return Ok(());
    }
    match presence_load()? {
        (_, Some(cfg)) => crate::out!(
            "设备统计：映射里的账号在这台 Mac 上用的时候（命令行会话、cca app 打开的 App），会报告给维护者的服务器 {}，配置了同一份映射的设备在账号页能看到这台 Mac 在不在用；给这些账号记下备查的 API key（cca key 账号）也经它加密同步给登录了同一个账号的其他设备，服务器看不到 key（都不想要：config.json 里加上 \"presence\": false）",
            cfg.at("url").py_str()
        ),
        (reason, None) => crate::out!("设备统计：{reason}"),
    }
    Ok(())
}

/// cca restore 之前：停掉这台 Mac 的上报进程（它先告诉服务器“停了”）
pub fn cmd_presence_stop(args: &[String]) -> R<()> {
    arity("cmd_presence_stop", args, 0, 0)?;
    let Some(j) = load_quiet(&path::join(&PRESENCE_DIR, "agent.json")).and_then(|j| j.as_dict()) else { return Ok(()) };
    // isinstance(pid, int)、type(started) is int、started 不是 0
    let Some(V::Int(started)) = j.get("started").filter(|s| s.truthy()) else { return Ok(()) };
    let pid = match j.get("pid") {
        Some(V::Int(pid)) => pid,
        Some(V::Bool(true)) => {
            // 布尔值在 Python 里也算 int：True 当成进程号 1 去问（ps 的参数写成 True，问不到），等于不在
            if !cfg!(target_os = "linux") {
                ps_lstart("True");
            }
            return Ok(());
        }
        _ => return Ok(()),
    };
    let alive = || cli_alive(pid, started, &proc_starts(&[pid]));
    if !alive() {
        return Ok(());
    }
    // SAFETY: 只是给一个进程号发信号
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
        return Ok(());
    }
    for _ in 0..50 {
        if !alive() {
            return Ok(());
        }
        sleep(0.1);
    }
    Ok(())
}
