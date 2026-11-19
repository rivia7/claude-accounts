//! 终端工具（命令行的 Claude Code、Codex）：只查版本，更新用它们自己的 update 命令（`cu_cli_run` 到 `cu_cli_install`）。

use super::feed::cu_cli_latest;
use super::pyv::*;
use super::{
    cu, cu_download, cu_run, cu_save_state, cu_target, cu_validate_plan, cu_version, cu_version_str, getenv, home, with_tempdir,
    ClientUpdateLock,
};
use crate::py::fs::gettempdir;
use crate::py::obj::{Dict, V};
use crate::py::proc::Done;
use crate::py::rand::token_hex;
use crate::py::time::time;
use crate::py::{path, text, uni, R};
use crate::{dict, re};

/// 运行 claude / codex：也从空环境开始（不带账号、代理、API key）。HOME 是真的：官方更新命令要往 ~/.local、~/.codex 里装；
/// PATH 里要有 ~/.local/bin：codex 的安装脚本发现它不在 PATH 里，会去改用户的 shell 配置文件
fn cu_cli_run(args: &[String], folder: &str, timeout: f64) -> R<Done> {
    let home = home();
    let mut env: Vec<(String, String)> = vec![
        (
            "PATH".into(),
            format!(
                "{}:{}",
                path::join_all(&[&home, ".local", "bin"]),
                getenv("PATH").unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".into())
            ),
        ),
        ("HOME".into(), home),
        ("TMPDIR".into(), folder.to_string()),
        ("LANG".into(), "en_US.UTF-8".into()),
        ("NO_COLOR".into(), "1".into()),
        ("TERM".into(), "dumb".into()),
    ];
    // env.update(CU.run_env(folder))
    for (k, v) in cu().run_env(folder)? {
        crate::py::proc::env_set(&mut env, &k, &v);
    }
    cu_run(args, folder, timeout, Some(env))
}

/// `re.search(r'(?<![\d.])\d+\.\d+\.\d+(?![\d.])', text)`：前后都不挨着数字和点的「数.数.数」，取最靠前的
pub(super) fn version_in(text: &str) -> Option<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let digit = |k: usize| chars.get(k).is_some_and(|(_, c)| uni::is_decimal(*c));
    let dot = |k: usize| chars.get(k).is_some_and(|(_, c)| *c == '.');
    let mut k = 0;
    while k < chars.len() {
        if !digit(k) || (k > 0 && (digit(k - 1) || dot(k - 1))) {
            k += 1;
            continue;
        }
        // 三段数字，中间各一个点
        let mut end = k;
        let mut ok = true;
        for part in 0..3 {
            if part > 0 {
                if !dot(end) {
                    ok = false;
                    break;
                }
                end += 1;
            }
            if !digit(end) {
                ok = false;
                break;
            }
            while digit(end) {
                end += 1;
            }
        }
        if ok && !digit(end) && !dot(end) {
            let from = chars[k].0;
            let to = chars.get(end).map_or(text.len(), |(i, _)| *i);
            return Some(&text[from..to]);
        }
        k += 1;
    }
    None
}

/// 现在装的版本
fn cu_cli_installed(target: &str, folder: &str) -> R<String> {
    let r = cu_cli_run(&[target.to_string(), "--version".to_string()], folder, 20.0)?;
    let said = String::from_utf8_lossy(&r.stdout);
    match version_in(&said).filter(|_| r.rc == 0) {
        Some(version) => Ok(version.to_string()),
        None => cu_error("blocked", format!("读不出 {} 的版本", cu().command)),
    }
}

/// 怎么装的：self（官方安装器，它的 update 命令自己换）、brew、npm，认不出的是 other；brew 时带上 cask 的名字
pub(super) fn cu_cli_method(target: &str) -> (&'static str, String) {
    let real = path::realpath(target);
    if let Some(cask) = re!(r"/Caskroom/([^/]+)/").search(&real) {
        return ("brew", cask.g(1).to_string());
    }
    if real.contains("/node_modules/") {
        return ("npm", String::new());
    }
    let own = format!("{}/", path::join(&home(), cu().own_dir));
    (if real.starts_with(&own) { "self" } else { "other" }, String::new())
}

/// 检查一种终端工具：现在的版本、官方最新的版本；有新的就记下（官方安装器装的给一个安装凭据，别的告诉用户在终端里运行什么）
pub fn cu_cli_check(state: &Dict, root: &str) -> R<()> {
    let cu = cu();
    let target = cu_target()?;
    state.set("checked_epoch", time());
    let body = || -> R<()> {
        cu_save_state(state)?;
        let (installed, method, cask, latest) = with_tempdir("check-", root, |folder| {
            let installed = cu_cli_installed(&target, folder)?;
            let (method, cask) = cu_cli_method(&target);
            let url = match method {
                "brew" => format!("https://formulae.brew.sh/api/cask/{cask}.json"),
                "npm" => cu.npm_url(),
                _ => cu.latest(),
            };
            let latest_file = path::join(folder, "latest");
            cu_download(&url, &latest_file, folder, false, None, &[])?;
            let latest = cu_cli_latest(&std::fs::read(&latest_file)?)?;
            Ok((installed, method, cask, latest))
        })?;
        state.set("metadata", dict! { "version" => latest.as_str() });
        if cu_version_str(&latest)? <= cu_version_str(&installed)? {
            state.pop("stage");
            state.set("status", "current");
            state.set("message", "已是最新版本");
            return Ok(());
        }
        state.set(
            "stage",
            dict! {
                "token" => token_hex(32), "method" => method, "metadata" => dict! { "version" => latest.as_str() },
                "target" => dict! { "path" => target.as_str(), "version" => installed.as_str() },
            },
        );
        state.set("status", "available");
        state.set(
            "message",
            if method == "self" {
                format!("可以直接更新到 {latest}")
            } else {
                format!("有新版本 {}：在终端里运行 {}", latest, cu.advice(method, &cask))
            },
        );
        Ok(())
    };
    let result = body();
    if let Err(e) = &result {
        if let Some((status, message)) = cu_parts(e) {
            state.set("status", status);
            state.set("message", message);
            state.pop("stage");
        }
    }
    cu_save_state(state)?; // finally
    result
}

/// status 用：现在的版本是现跑的（claude 平时会自己更新，缓存的结论可能已经过时了），别的来自上次检查
pub fn cu_cli_view(state: &Dict, problem: Option<&(String, String)>) -> R<Dict> {
    let cu = cu();
    let target = cu_target()?;
    let row = dict! {
        "id" => cu.id, "label" => cu.label, "kind" => "cli", "path" => if target.is_empty() { V::None } else { V::from(target.as_str()) },
        "installed_version" => V::None, "available_version" => V::None, "staged_version" => V::None,
        "package_version" => V::None, "package_build" => V::None, "install_token" => V::None, "can_install" => false,
        "status" => state.get("status").unwrap_or_else(|| V::from("unchecked")), "message" => state.get("message").unwrap_or_else(|| V::from("尚未检查官方更新")),
        "checked_epoch" => state.get("checked_epoch"), "auto_install_pending" => false,
    };
    if target.is_empty() {
        row.set("status", "missing");
        row.set("message", format!("没有找到 {} 命令", cu.command));
        return Ok(row);
    }
    let fill = || -> R<()> {
        let installed = with_tempdir("cca-cli-", &gettempdir(), |folder| cu_cli_installed(&target, folder))?;
        row.set("installed_version", installed.as_str());
        let (metadata, stage) = (state.get("metadata"), state.get("stage"));
        let latest = match &metadata {
            Some(V::Dict(m)) => m.get("version"),
            _ => None,
        };
        row.set("available_version", latest.clone());
        if latest.as_ref().is_some_and(|l| l.truthy())
            && cu_version(latest.as_ref().expect("上面看过"))? <= cu_version_str(&installed)?
        {
            // 刚更新过的留着“已更新到 X”那句话
            if !matches!(state.get("status"), Some(V::Str(s)) if &*s == "updated") {
                row.set("status", "current");
                row.set("message", "已是最新版本");
            }
        } else if let Some(V::Dict(stage)) = &stage {
            let stage_v = V::Dict(stage.clone());
            if matches!(stage.get("method"), Some(V::Str(m)) if &*m == "self")
                && sub2(&stage_v, "target", "path")?.eq_py(&V::from(target.as_str()))
            {
                row.set("install_token", at(stage, "token")?);
                row.set("can_install", true);
            }
        }
        Ok(())
    };
    match fill() {
        Ok(()) => {}
        Err(e) if caught(&e) => {
            let (status, message) = cu_parts(&e).unwrap_or(("blocked", "读不出这个命令的状态"));
            row.set("status", status);
            row.set("message", message);
        }
        Err(e) => return Err(e),
    }
    if let Some((status, message)) = problem {
        row.set("status", status.as_str());
        row.set("message", message.as_str());
        row.set("install_token", V::None);
        row.set("can_install", false);
    }
    Ok(row)
}

/// 确认和安装之前：还是这个命令、用官方安装器装的、确实比现在的新。返回的 target 带着现在的版本（确认框里写）
pub fn cu_cli_validate(stage: &Dict) -> R<Dict> {
    let cu = cu();
    let stage_v = V::Dict(stage.clone());
    let target = cu_target()?;
    // not path or path != stage['target']['path'] or stage.get('method') != 'self'
    let moved = target.is_empty()
        || !sub2(&stage_v, "target", "path")?.eq_py(&V::from(target.as_str()))
        || !matches!(stage.get("method"), Some(V::Str(m)) if &*m == "self");
    if moved {
        return cu_error("needs_recheck", format!("{} 的安装位置变了，请重新检查", cu.command));
    }
    let installed = with_tempdir("cca-cli-", &gettempdir(), |folder| cu_cli_installed(&target, folder))?;
    if cu_version(&sub2(&stage_v, "metadata", "version")?)? <= cu_version_str(&installed)? {
        return cu_error("needs_recheck", format!("{} 已经是这个版本或者更新的了", cu.command));
    }
    // dict(stage, target=dict(stage['target'], version=installed))
    let new_target = match at(stage, "target")? {
        V::Dict(t) => t.copy(),
        other => return type_error(format!("'{}' object is not iterable", type_name(&other))),
    };
    new_target.set("version", installed);
    let out = stage.copy();
    out.set("target", new_target);
    Ok(out)
}

/// 用户同意之后运行官方的 update 命令。官方安装器把版本装在各自的目录里再换链接：开着的会话继续用旧版本
pub fn cu_cli_install(state: &Dict, token: &V, root: &str) -> R<()> {
    let cu = cu();
    let _lock = ClientUpdateLock::acquire(&path::join(root, "install.lock"))?;
    let stage = cu_validate_plan(state, token)?;
    let (target, want) = (path_str(&sub2(&stage, "target", "path")?)?, sub2(&stage, "metadata", "version")?);
    let (r, installed) = with_tempdir("update-", root, |folder| {
        let r = match cu_cli_run(&[target.clone(), "update".to_string()], folder, 600.0) {
            Ok(r) => r,
            Err(e) if is_cu(&e) => return cu_error("error", format!("{} update 运行失败或超时，稍后再试", cu.command)),
            Err(e) => return Err(e),
        };
        let installed = cu_cli_installed(&target, folder)?;
        Ok((r, installed))
    })?;
    if cu_version_str(&installed)? < cu_version(&want)? {
        // (r.stderr or r.stdout).decode('utf-8', 'replace').strip().splitlines()
        let said = String::from_utf8_lossy(if r.stderr.is_empty() { &r.stdout } else { &r.stderr }).into_owned();
        let lines = text::splitlines(text::strip(&said));
        let why = lines.last().map_or(String::new(), |last| format!("：{}", text::head(last, 200)));
        return cu_error("error", format!("{} update 没有更新成功{}", cu.command, why));
    }
    state.pop("stage");
    state.set("metadata", dict! { "version" => installed.as_str() });
    state.set("status", "updated");
    state.set("message", format!("已更新到 {installed}，下次启动时用新版本"));
    cu_save_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_search_like_the_regex() {
        // 右边是原来那个带前瞻、后顾的正则给的（CPython 3.14）
        for (text, want) in [
            ("2.0.0 (Claude Code)", Some("2.0.0")),
            ("codex-cli 0.166.0", Some("0.166.0")),
            ("v1.2.3", Some("1.2.3")),
            ("1.2.3.4", None),
            ("1.2.3.4 then 5.6.7", Some("5.6.7")),
            ("x1.2.3-beta", Some("1.2.3")),
            ("12.34.56", Some("12.34.56")),
            ("1.2", None),
            ("1.2.", None),
            (".1.2.3", None),
            ("1..2.3 4.5.6", Some("4.5.6")),
            ("a1.2.3b", Some("1.2.3")),
            ("1.2.3.", None),
            ("١.٢.٣", Some("١.٢.٣")),
            ("", None),
            ("10.20.30\n", Some("10.20.30")),
            ("9 1.2.3", Some("1.2.3")),
            ("1.2.x 3.4.5", Some("3.4.5")),
        ] {
            assert_eq!(version_in(text), want, "{text:?}");
        }
    }
}
