//! 设备统计（`presence_problems` 到 `cmd_presence_stop`、`past_time` 到 `cmd_presence_note`）：presence-agent、presence-wake、
//! presence-stop、presence-view、presence-note。
//!
//! 子命令还没移植；`presence_problems`（映射的严格检查要用）已经移植。

use super::Cmd;
use crate::py::obj::V;
use crate::py::re::Re;
use crate::py::text;
use std::sync::LazyLock;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[];

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
