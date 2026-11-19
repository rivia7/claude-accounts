//! 替换桌面客户端：核对进程、无覆盖的改名、事务记录（transaction.json）和中断以后的恢复（`cu_no_processes` 到 `cu_install`）。

use super::pyv::*;
use super::{
    bytes_strip, cli, cu, cu_build, cu_bundle, cu_content, cu_private, cu_root, cu_run, cu_save_state, cu_snapshot,
    cu_validate_plan, cu_verify_archive, extract, strs, ClientUpdateLock,
};
use crate::py::fs::{self, load_quiet, save_json};
use crate::py::obj::{Dict, V};
use crate::py::shutil::{self, TemporaryDirectory};
use crate::py::{path, text, uni, Fail, R};
use crate::{dict, re};
use std::collections::HashMap;
use std::os::unix::fs::OpenOptionsExt;

/// `re.match(r'^\s*(\d+)\s+(.+)$', line)`：ps 的一行 → (进程号, 后面的内容)。不经正则库（`\d` 的 Unicode 表和 Python 的不是同一版）
pub(super) fn ps_line(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim_start_matches(uni::is_space);
    let digits = rest.find(|c: char| !uni::is_decimal(c)).unwrap_or(rest.len());
    if digits == 0 {
        return None;
    }
    let (pid, after) = rest.split_at(digits);
    let tail = after.trim_start_matches(uni::is_space);
    let gap = &after[..after.len() - tail.len()];
    if gap.is_empty() {
        return None;
    }
    if !tail.is_empty() {
        return Some((pid, tail));
    }
    // 后面全是空白：`\s+` 让出最后一个给 `(.+)`
    let last = gap.char_indices().next_back().expect("gap 不是空的").0;
    (last > 0).then(|| (pid, &gap[last..]))
}

/// `re.search(r'(?:^|[\s=\x22\x27])' + re.escape(path) + r'(?:[/\s\x22\x27]|$)', command)`：命令行里有没有这个路径
/// （前面是开头、空白、等号或引号，后面是结尾、斜杠、空白或引号）
pub(super) fn path_in_command(path: &str, command: &str) -> bool {
    let before_ok = |i: usize| command[..i].chars().next_back().is_none_or(|c| uni::is_space(c) || matches!(c, '=' | '"' | '\''));
    let after_ok = |i: usize| command[i..].chars().next().is_none_or(|c| uni::is_space(c) || matches!(c, '/' | '"' | '\''));
    let mut from = 0;
    while from <= command.len() {
        let Some(k) = command[from..].find(path) else { return false };
        let i = from + k;
        if before_ok(i) && after_ok(i + path.len()) {
            return true;
        }
        // 下一个可能的起点：挨着的也要看（会重叠的）
        match command[i..].chars().next() {
            Some(c) => from = i + c.len_utf8(),
            None => return false,
        }
    }
    false
}

/// 没有进程在用这些路径、这个产品的更新器也没在跑，才能动程序；有就是 running
pub fn cu_no_processes(paths: &[V]) -> R<()> {
    let cu = cu();
    let executables = cu_run(&strs(&["/bin/ps", "-axww", "-o", "pid=,comm="]), "/private/tmp", 20.0, None)?;
    let result = cu_run(&strs(&["/bin/ps", "-axww", "-o", "pid=,command="]), "/private/tmp", 20.0, None)?;
    if executables.rc != 0
        || bytes_strip(&executables.stdout).is_empty()
        || result.rc != 0
        || bytes_strip(&result.stdout).is_empty()
    {
        return cu_error("blocked", "无法核验客户端进程，未替换程序");
    }
    let mut commands: HashMap<String, String> = HashMap::new();
    for line in text::splitlines(&String::from_utf8_lossy(&executables.stdout)) {
        let Some((pid, comm)) = ps_line(line) else {
            return cu_error("blocked", "客户端进程列表无法完整解析，未替换程序");
        };
        commands.insert(pid.to_string(), comm.to_string());
    }
    for line in text::splitlines(&String::from_utf8_lossy(&result.stdout)) {
        let Some((pid, command)) = ps_line(line) else {
            return cu_error("blocked", "客户端进程列表无法完整解析，未替换程序");
        };
        let mut target_match = false;
        for p in paths {
            // re.escape(path)：不是文字是 TypeError
            let V::Str(p) = p else {
                return type_error(format!("decoding to str: need a bytes-like object, {} found", type_name(p)));
            };
            if path_in_command(p, command) {
                target_match = true;
                break;
            }
        }
        let executable = commands.get(pid).map_or("", |s| s.as_str());
        let updater = executable.contains(cu.bundle) && cu.updater.iter().any(|word| executable.contains(word));
        if target_match || updater {
            return cu_error("running", format!("请先退出所有 {} 窗口及后台进程，再确认安装", cu.label));
        }
    }
    Ok(())
}

/// Darwin 的无覆盖原子 rename；检查后突然出现的目标也不能被覆盖。
pub fn cu_rename_exclusive(source: &str, destination: &str) -> R<()> {
    #[cfg(test)]
    if let Some(r) = super::tests::on_rename(source, destination) {
        return r;
    }
    rename_exclusive(source, destination)
}

#[cfg(target_os = "macos")]
pub(super) fn rename_exclusive(source: &str, destination: &str) -> R<()> {
    let (Ok(from), Ok(to)) = (std::ffi::CString::new(source), std::ffi::CString::new(destination)) else {
        return cu_error("blocked", "无法安全移动客户端程序");
    };
    // SAFETY: from、to 是以 0 结尾的路径，调用期间都活着
    if unsafe { libc::renameatx_np(libc::AT_FDCWD, from.as_ptr(), libc::AT_FDCWD, to.as_ptr(), libc::RENAME_EXCL) } != 0 {
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EEXIST | libc::ENOTEMPTY) => cu_error("needs_recheck", "安装目标已由其他更新器创建，未覆盖它"),
            Some(libc::EACCES | libc::EPERM | libc::EROFS) => cu_error("blocked", "没有替换此客户端的权限；未请求管理员权限"),
            _ => cu_error("blocked", "无法安全移动客户端程序"),
        };
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(super) fn rename_exclusive(_source: &str, _destination: &str) -> R<()> {
    cu_error("blocked", "系统缺少安全的无覆盖安装接口")
}

fn cu_sync_directory(path: &str) -> R<()> {
    let dir = std::fs::File::open(path)?;
    fs::fsync(&dir)?;
    Ok(())
}

fn cu_journal(data: &Dict) -> R<()> {
    let path = path::join(&cu_root(true)?, "transaction.json");
    if path::lexists(&path) {
        cu_private(&path, false, false)?;
    }
    save_json(&path, &V::Dict(data.clone()), Some(0o600), false)?;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(&path)?;
    fs::fsync(&file)?;
    drop(file);
    cu_sync_directory(&path::dirname(&path))
}

/// rename 改变根目录的路径/ctime；程序内容、文件身份和目录 inode 必须保持原样。
fn cu_moved_matches(current: &Dict, before: &V) -> R<bool> {
    for k in ["version", "build", "hashes", "files", "device", "inode", "modified_ns"] {
        let (a, b) = (current.get(k), get(before, k)?);
        let same = match (&a, &b) {
            (Some(a), Some(b)) => a.eq_py(b),
            (None, None) => true,
            (Some(x), None) | (None, Some(x)) => x.is_none(),
        };
        if !same {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `shutil.rmtree(work)`
fn rmtree(work: &str) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(r) = super::tests::on_rmtree(work) {
        return r;
    }
    shutil::rmtree(work)
}

fn cu_cleanup_work(work: &str, state: &Dict) -> R<()> {
    if rmtree(work).is_err() {
        state.set("retained_recovery_path", work);
        cu_save_state(state)?;
    }
    Ok(())
}

/// 只在 manager/install 锁内调用；status 仅显示待恢复，不触碰事务现场。
pub fn cu_recover(state: &Dict) -> R<()> {
    let cu = cu();
    let root = cu_root(true)?;
    let journal = path::join(&root, "transaction.json");
    if !path::lexists(&journal) {
        return Ok(());
    }
    cu_private(&journal, false, false)?;
    let data = match load_quiet(&journal).and_then(|d| d.as_dict()) {
        Some(data) if data.get("v").is_some_and(|v| v.eq_py(&V::Int(1))) => data,
        _ => return cu_error("blocked", "安装恢复记录无法识别，已保留现场"),
    };
    // target = data.get('target', {}).get('path', '')；work = data.get('work', '')
    let target = match data.get("target") {
        None => V::from(""),
        Some(t) => get(&t, "path")?.unwrap_or_else(|| V::from("")),
    };
    let work = data.get("work").unwrap_or_else(|| V::from(""));
    let target = path_str(&target)?; // os.path.isabs(target)
    let verified = (|| -> R<Option<String>> {
        if !target.starts_with('/') {
            return Ok(None);
        }
        if target.contains('\0') {
            return Err(Fail::Value("embedded null byte".into())); // os.path.realpath 里的 lstat
        }
        if target != path::realpath(&target) || !work.truthy() {
            return Ok(None);
        }
        let work = path_str(&work)?; // os.path.dirname(work)
        if path::dirname(&work) != path::dirname(&target)
            || re!(r"\.cca-client-install-[a-z0-9_]+").fullmatch(path::basename(&work)).is_none()
        {
            return Ok(None);
        }
        if !eq_opt(&V::from(path::join(&work, "previous.app")), data.get("backup")) {
            return Ok(None);
        }
        let Some(app) = cu.app else {
            return attr_error("'CuCli' object has no attribute 'app'");
        };
        Ok(eq_opt(&V::from(path::join(&work, app)), data.get("incoming")).then_some(work))
    })()?;
    let Some(work) = verified else {
        return cu_error("blocked", "安装恢复路径无法核验，已保留现场");
    };
    cu_private(&work, true, false)?;
    let (backup, incoming) = (path::join(&work, "previous.app"), path::join(&work, cu.app.unwrap_or("")));
    let mut cleanup_work = false;
    let all_three = [V::from(target.as_str()), V::from(backup.as_str()), V::from(incoming.as_str())];
    cu_no_processes(&all_three)?;
    let recorded = at(&data, "target"); // data['target']：用到时才看有没有
    if path::lexists(&target) {
        let current = cu_snapshot(&target, true)?;
        let current_v = V::Dict(current.clone());
        if eq_opt(&V::Dict(cu_content(&current_v)?), data.get("candidate")) && path::lexists(&backup) {
            cu_bundle(&target, &root, Some(&at(&data, "metadata")?))?;
            // 崩溃可能发生在新程序到位后、状态保存前；不再移动已完成的目标。
            state.pop("stage");
            state.set("status", "updated");
            state.set("message", "官方客户端更新已安装");
            cleanup_work = true;
        } else if current_v.eq_py(&recorded?) && !path::lexists(&backup) {
            // 移走旧程序前中断：当前原程序未变化，可以清除仅属于本事务的候选目录。
            state.set("status", "needs_recheck");
            state.set("message", "上次安装未开始替换，请重新检查更新");
            cleanup_work = true;
        } else {
            let current = cu_bundle(&target, &root, None)?;
            if cu_build(&current.at("build"))? < cu_build(&sub(&at(&data, "target")?, "build")?)? {
                return cu_error("needs_recheck", "目标版本异常，已保留安装现场且未覆盖程序");
            }
            state.pop("stage");
            state.set("status", "needs_recheck");
            state.set("message", "已保留原生更新器安装的客户端，请重新检查");
            if path::lexists(&backup) {
                state.set("retained_recovery_path", work.as_str()); // 竞态现场不自动删除。
            } else {
                cleanup_work = true;
            }
        }
    } else if path::lexists(&backup) {
        let original = cu_bundle(&backup, &root, None)?;
        let recorded = recorded?;
        if !cu_moved_matches(&original, &recorded)? && cu_build(&original.at("build"))? < cu_build(&sub(&recorded, "build")?)? {
            return cu_error("needs_recheck", "备份版本异常，已保留现场且未自动恢复");
        }
        cu_no_processes(&all_three)?;
        cu_rename_exclusive(&backup, &target)?;
        cu_sync_directory(&path::dirname(&target))?;
        cu_sync_directory(&work)?;
        state.pop("stage");
        state.set("status", "needs_recheck");
        state.set("message", "已恢复可核验的官方客户端，请重新检查更新");
        cleanup_work = true;
    } else {
        return cu_error("blocked", "未找到可核验的客户端或备份，已保留安装恢复记录");
    }
    cu_save_state(state)?;
    std::fs::remove_file(&journal)?;
    cu_sync_directory(&root)?;
    if cleanup_work {
        cu_cleanup_work(&work, state)?;
    }
    Ok(())
}

/// `os.access(p, os.W_OK)`
fn writable(p: &str) -> bool {
    match std::ffi::CString::new(p) {
        // SAFETY: c 以 0 结尾
        Ok(c) => unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 },
        Err(_) => false,
    }
}

/// 用户确认以后装上暂存的更新（token 是确认时给的安装凭据）。装完 state 里记着结果
pub fn cu_install(state: &Dict, token: &V) -> R<()> {
    let root = cu_root(true)?;
    if cu().cli {
        return cli::cu_cli_install(state, token, &root);
    }
    let _lock = ClientUpdateLock::acquire(&path::join(&root, "install.lock"))?;
    cu_recover(state)?;
    let stage = cu_validate_plan(state, token)?;
    let target_v = sub2(&stage, "target", "path")?;
    let target = path_str(&target_v)?;
    cu_no_processes(std::slice::from_ref(&target_v))?;
    if !writable(&path::dirname(&target)) {
        return cu_error("blocked", "客户端所在目录不可写；请使用原版安装器更新");
    }
    // 同一文件系统内准备候选程序，整个准备阶段尚未移动目标。
    let work = {
        let tmp = TemporaryDirectory::new(".cca-client-install-", &path::dirname(&target))?;
        let name = tmp.name.clone();
        std::mem::forget(tmp); // tempfile.mkdtemp：不自动删
        name
    };
    let mut journal_written = false;
    let body = |journal_written: &mut bool| -> R<()> {
        let archive = path::join(&root, "package.zip");
        let (metadata, staged_target) = (sub(&stage, "metadata")?, sub(&stage, "target")?);
        if !V::from(cu_verify_archive(&archive, &metadata, &work)?).eq_py(&sub(&stage, "package_sha256")?) {
            return cu_error("needs_recheck", "暂存安装包已变化，请重新检查");
        }
        let incoming = extract::cu_extract(&archive, &work)?;
        let candidate = V::Dict(cu_bundle(&incoming, &work, Some(&metadata))?);
        if !V::Dict(cu_content(&candidate)?).eq_py(&sub(&stage, "package")?) {
            return cu_error("needs_recheck", "解压程序与此前确认的安装包不一致");
        }
        if !V::Dict(cu_bundle(&target, &work, None)?).eq_py(&staged_target) {
            return cu_error("needs_recheck", "原生更新器已改变客户端，未覆盖它");
        }
        cu_validate_plan(state, token)?;
        let both = [target_v.clone(), V::from(incoming.as_str())];
        cu_no_processes(&both)?;
        let backup = path::join(&work, "previous.app");
        let transaction = dict! {
            "v" => 1, "target" => staged_target.clone(), "candidate" => cu_content(&candidate)?, "metadata" => metadata.clone(),
            "incoming" => incoming.as_str(), "backup" => backup.as_str(), "work" => work.as_str(),
        };
        cu_journal(&transaction)?;
        *journal_written = true;
        // 日志已持久化；再核验目标，移动之后也检查被移走的正是刚才核验的版本。
        if !V::Dict(cu_snapshot(&target, true)?).eq_py(&staged_target) {
            return cu_error("needs_recheck", "客户端在提交前发生变化，未覆盖它");
        }
        cu_no_processes(&both)?;
        cu_rename_exclusive(&target, &backup)?;
        cu_sync_directory(&path::dirname(&target))?;
        cu_sync_directory(&work)?;
        if !cu_moved_matches(&cu_snapshot(&backup, true)?, &staged_target)? {
            return cu_error("needs_recheck", "原生更新器在替换时改变了客户端，已保留现场");
        }
        cu_no_processes(&[target_v.clone(), V::from(backup.as_str()), V::from(incoming.as_str())])?;
        cu_rename_exclusive(&incoming, &target)?;
        cu_sync_directory(&path::dirname(&target))?;
        cu_sync_directory(&work)?;
        if !V::Dict(cu_content(&V::Dict(cu_snapshot(&target, true)?))?).eq_py(&transaction.at("candidate")) {
            return cu_error("needs_recheck", "客户端在安装后被其他更新器改变，已保留现场");
        }
        state.pop("stage");
        state.set("status", "updated");
        state.set("message", "官方客户端更新已安装");
        cu_save_state(state)?;
        std::fs::remove_file(path::join(&root, "transaction.json"))?;
        cu_sync_directory(&root)?;
        cu_cleanup_work(&work, state) // 提交完成才清理，不累积每版约 2 GB 的程序副本。
    };
    let mut result = body(&mut journal_written);
    if let Err(e) = &result {
        // except Exception：记过事务的先照记录恢复
        if is_exception(e) && journal_written {
            match cu_recover(state) {
                Err(r) if !caught(&r) => result = Err(r),
                _ => {} // 无法证明安全就保留 journal；下次 check 再恢复，不覆盖新目标。
            }
        }
    }
    // finally
    if !journal_written {
        rmtree(&work)?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_lines_like_the_regex() {
        // 右边是原来那个正则给的（CPython 3.14）
        assert_eq!(ps_line("  123 /sbin/launchd"), Some(("123", "/sbin/launchd")));
        assert_eq!(ps_line("1 a b  c "), Some(("1", "a b  c ")));
        assert_eq!(ps_line("1  "), Some(("1", " ")));
        assert_eq!(ps_line("1 \t"), Some(("1", "\t")));
        assert_eq!(ps_line("1 "), None);
        assert_eq!(ps_line("1"), None);
        assert_eq!(ps_line("12a b"), None);
        assert_eq!(ps_line("a 1 b"), None);
        assert_eq!(ps_line(""), None);
        assert_eq!(ps_line("   "), None);
        assert_eq!(ps_line("\u{3000}٣٤\u{a0}x"), Some(("٣٤", "x")));
        assert_eq!(ps_line("7 <defunct>"), Some(("7", "<defunct>")));
    }

    #[test]
    fn paths_in_command_lines() {
        let app = "/Applications/ChatGPT.app";
        for (command, want) in [
            ("/Applications/ChatGPT.app/Contents/MacOS/ChatGPT", true),
            ("/Applications/ChatGPT.app", true),
            ("open -a /Applications/ChatGPT.app --args x", true),
            ("x --path=/Applications/ChatGPT.app", true),
            ("x \"/Applications/ChatGPT.app\"", true),
            ("x '/Applications/ChatGPT.app'", true),
            ("/Applications/ChatGPT.app2/Contents/MacOS/x", false),
            ("/tmp/Applications/ChatGPT.app/x", false),
            ("x:/Applications/ChatGPT.app", false),
            ("/Applications/ChatGPT.apparently /Applications/ChatGPT.app", true),
            ("", false),
        ] {
            assert_eq!(path_in_command(app, command), want, "{command:?}");
        }
        // 会重叠的
        assert!(path_in_command("a a", "xa a a"));
        assert!(!path_in_command("a a", "xa ab"));
    }
}
