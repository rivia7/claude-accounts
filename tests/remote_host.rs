//! 远程会话的会话宿主和接入端（docs/dev/remote.md）：只运行 cca 的辅助程序（cli/），都在临时 HOME 里、真的伪终端上。
//!
//! 宿主（remote-host）顶替客户端的位置：客户端跑在它开的 pty 里，环境、参数、退出码原样；启动它的终端没了会话照样运行；
//! 接入端（remote-attach）经 ~/.cca/remote/ 里的 socket 接进来。客户端是 cca-test fake-client（一行一条命令，见
//! tests/tool/fake_client.rs），终端是 kit::term 的 Term。
//!
//! 每个用例跑两遍，各是一个子用例：RemoteHost（平常的 ~/.cca）和 DeepHome（~/.cca 的路径很长，socket 的地址放不下整个
//! 路径：照样能开、能列、能接入）。原来是两个类，DeepHome 继承 RemoteHost 的全部用例、只把 ~/.cca 换成很深的目录。

use kit::fsx::{self, wait_for};
use kit::harness::{self, Ctx, Module};
use kit::json::{self, Style, Value};
use kit::proc::{Cmd, Output};
use kit::term::{self, Term};
use kit::{case, list};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const MODULE: Module = Module {
    name: "remote_host",
    src: include_str!("remote_host.rs"),
    default: true,
    tests: &[
        case!(client_runs_as_before_and_exit_code_comes_back),
        case!(environment_is_untouched),
        case!(killed_client_and_forwarded_signal),
        case!(slow_terminal_loses_nothing_when_the_client_exits),
        case!(client_closes_the_terminal_and_runs_on),
        case!(without_a_terminal_the_client_runs_directly),
        case!(terminal_gone_session_goes_on_and_can_be_attached),
        case!(two_terminals_share_one_session),
        case!(only_the_primary_terminal_gets_what_is_not_plainly_safe),
        case!(late_viewer_gets_the_modes_and_puts_its_terminal_back),
        case!(attach_from_a_terminal_the_app_owns),
        case!(attach_errors),
        case!(end_a_session),
        case!(a_host_that_does_not_answer_is_not_cleaned_up),
        case!(switch),
    ],
};

/// 一个用例的临时 HOME 和里面的东西
struct Fx {
    /// ~/.cca；DeepHome 里是 HOME 下面很深的一个目录，经 CCA_HOME 告诉辅助程序
    cca: PathBuf,
    /// 会话起在这个目录里
    work: PathBuf,
    /// 假客户端：写成脚本的 cca-test fake-client
    client: String,
    /// 给辅助程序的环境：就这几个
    env: Vec<(String, String)>,
}

impl Fx {
    fn new(t: &mut Ctx, deep: bool) -> Fx {
        let home = t.tmp("cca-remote-");
        let cca = if deep { home.join("d".repeat(60)).join("e".repeat(60)).join(".cca") } else { home.join(".cca") };
        std::fs::create_dir_all(&cca).unwrap();
        fsx::write(cca.join("config.json"), "{}");
        let work = home.join("项目 one");
        std::fs::create_dir(&work).unwrap();
        let client = term::write_client(&home);
        let text = |path: &PathBuf| path.to_str().expect("临时目录的路径是 UTF-8").to_string();
        let mut env = vec![
            ("HOME".to_string(), text(&home)),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("MARK".to_string(), "kept".to_string()),
        ];
        if deep {
            env.push(("CCA_HOME".to_string(), text(&cca)));
        }
        Fx { client: text(&client), cca, work, env }
    }

    /// 辅助程序的一条命令：只有 env 里那几个环境变量，在 work 里
    fn cmd(&self, args: &[&str]) -> Cmd {
        let mut cmd = Cmd::new(harness::helper());
        cmd.args(args).envs(self.env.iter().map(|(k, v)| (k, v))).cwd(&self.work);
        cmd
    }

    /// 在一个新的终端里运行辅助程序
    fn term(&self, args: &[&str], rows: u16, cols: u16) -> Term {
        Term::start(&mut self.cmd(args), rows, cols)
    }

    /// 起一个会话：宿主在 24 行 80 列的终端里，客户端是假客户端
    fn host(&self, client_args: &[&str]) -> Term {
        let mut args = vec!["remote-host", "claude", "工作(alice@example.com)", "工作", "--", self.client.as_str()];
        args.extend(client_args);
        self.term(&args, 24, 80)
    }

    fn attach(&self, args: &[&str]) -> Term {
        self.attach_sized(args, 24, 80)
    }

    fn attach_sized(&self, args: &[&str], rows: u16, cols: u16) -> Term {
        let mut argv = vec!["remote-attach"];
        argv.extend(args);
        self.term(&argv, rows, cols)
    }

    /// 不在终端里运行辅助程序，输出收回来
    fn run_helper(&self, args: &[&str]) -> Output {
        self.cmd(args).timeout(Duration::from_secs(15)).output().unwrap_or_else(|e| panic!("{e}"))
    }

    /// 会话目录里现在有哪些文件
    fn files(&self) -> Vec<String> {
        let Ok(dir) = std::fs::read_dir(self.cca.join("remote")) else { return Vec::new() };
        let mut names: Vec<String> = dir.map(|e| e.unwrap().file_name().to_str().unwrap().to_string()).collect();
        names.sort();
        names
    }

    /// 这个宿主进程的会话说明
    fn metas(&self, pid: i32) -> Vec<Value> {
        let dir = self.cca.join("remote");
        let names = self.files().into_iter().filter(|n| n.ends_with(".json"));
        names.map(|n| fsx::read_json(dir.join(n))).filter(|m| m["pid"] == Value::from(pid)).collect()
    }
}

/// 一个用例在两种 ~/.cca 上各跑一遍
fn both(t: &mut Ctx, body: impl Fn(&Fx)) {
    for (class, deep) in [("RemoteHost", false), ("DeepHome", true)] {
        t.sub(class, |t| body(&Fx::new(t, deep)));
    }
}

/// 字节里有没有这一截
fn has(hay: &[u8], needle: impl AsRef<[u8]>) -> bool {
    term::find(hay, needle).is_some()
}

/// Python 的 re.search(pattern, text) 找不找得到。正则照原来的写；只有末尾的 $ 换一种写法：Python 里它在最后那个换行的
/// 前面也算数，regex 这个库的 $ 只认真正的末尾
fn matches(pattern: &str, text: &str) -> bool {
    let pattern = match pattern.strip_suffix('$') {
        Some(head) => format!("{head}\n?\\z"),
        None => pattern.to_string(),
    };
    regex::Regex::new(&pattern).unwrap_or_else(|e| panic!("正则 {pattern:?} 写得不对：{e}")).is_match(text)
}

/// 正好一个
fn only(mut metas: Vec<Value>) -> Value {
    assert_eq!(metas.len(), 1, "{metas:?}");
    metas.remove(0)
}

const NO_FILES: [&str; 0] = [];

fn client_runs_as_before_and_exit_code_comes_back(t: &mut Ctx) {
    both(t, |fx| {
        let mut h = fx.host(&["--flag", "two words"]);
        h.expect(r"<ready 24 80 leader=True ctty=True tty=True args=\['--flag', 'two words'\]>");
        // 会话目录只给自己，socket 和说明各一个；说明里是哪个账号、哪个文件夹，没有别的
        let d = fx.cca.join("remote");
        assert_eq!(fsx::mode(&d), 0o700);
        let names = fx.files();
        assert_eq!(names.iter().map(|n| n.split('.').nth(1).unwrap()).collect::<Vec<_>>(), ["json", "sock"]);
        assert_eq!(names.iter().map(|n| fsx::mode(d.join(n))).collect::<BTreeSet<_>>(), BTreeSet::from([0o600]));
        let meta = fsx::read_json(d.join(&names[0]));
        assert_eq!(
            list![&meta["v"], &meta["tool"], &meta["key"], &meta["name"], &meta["cwd"], &meta["pid"]],
            list![1, "claude", "工作(alice@example.com)", "工作", fx.work.to_str().unwrap(), h.pid()]
        );
        assert_eq!(
            meta.obj().keys().collect::<BTreeSet<_>>(),
            BTreeSet::from(["v", "sid", "pid", "tool", "key", "name", "cwd", "since"])
        );
        assert_eq!(names[0], format!("{}.json", &meta["sid"].str()[..12]));
        // 按键过去、输出回来（中文也一样）
        h.send("say 你好\r");
        h.expect("<你好>");
        // 窗口大小变了，客户端跟着变
        h.resize(30, 100);
        h.expect("<size 30 100>");
        h.send("exit 7\r");
        assert_eq!(h.wait(), 7);
        assert_eq!(fx.files(), NO_FILES, "退出以后 socket 和说明都删掉");
    });
}

fn environment_is_untouched(t: &mut Ctx) {
    both(t, |fx| {
        let mut t = fx.term(&["remote-host", "claude", "k", "n", "--", "/usr/bin/env"], 24, 80);
        assert_eq!(t.wait(), 0);
        let text = t.text().replace('\r', "");
        let mut got: Vec<&str> = text.split('\n').filter(|line| !line.is_empty()).collect();
        got.sort();
        let mut want: Vec<String> = fx.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        want.sort();
        assert_eq!(got, want, "宿主不给客户端加、减环境变量");
    });
}

fn killed_client_and_forwarded_signal(t: &mut Ctx) {
    both(t, |fx| {
        let mut h = fx.host(&[]);
        h.expect("<ready");
        h.send("kill\r");
        assert_eq!(h.wait(), 128 + libc::SIGKILL, "客户端被信号杀掉：128 + 信号");
        let mut h = fx.host(&[]);
        h.expect("<ready");
        h.signal(libc::SIGTERM); // 发给宿主的转给客户端
        h.expect("<term>");
        assert_eq!(h.wait(), 5);
    });
}

fn slow_terminal_loses_nothing_when_the_client_exits(t: &mut Ctx) {
    both(t, |fx| {
        let mut h = fx.host(&[]);
        h.expect("<ready");
        let mark = h.mark();
        h.send("dump\r");
        std::thread::sleep(Duration::from_millis(1500)); // 终端先不收：客户端写了两兆多就退出，宿主要等终端把它们都收完
        h.expect_in("<dumped>", Duration::from_secs(30), mark);
        assert_eq!(h.wait(), 6);
        // pty 把 \n 换成 \r\n，所以是 \r\r\n
        let rx = regex::bytes::Regex::new(r"(?-u)dump (\d{7}) \.+\r+\n").unwrap();
        let lines: Vec<u32> =
            rx.captures_iter(&h.out()[mark..]).map(|c| std::str::from_utf8(&c[1]).unwrap().parse().unwrap()).collect();
        let wrong = lines.iter().zip(0u32..).find(|(got, want)| *got != want);
        assert!(
            lines.len() == 40000 && wrong.is_none(),
            "一行不少、先后不乱：收到 {} 行，头一处不对的（收到的, 该是的）：{wrong:?}",
            lines.len()
        );
    });
}

fn client_closes_the_terminal_and_runs_on(t: &mut Ctx) {
    both(t, |fx| {
        let mut h = fx.host(&[]);
        h.expect("<ready");
        h.send("closetty\r");
        h.expect("<closing>");
        assert_eq!(h.wait(), 4, "客户端关了终端还在跑：等到它真的退出，退出码照旧");
        assert!(h.cpu() < Duration::from_millis(500), "等的这一秒多不能空转：用了 {:?}", h.cpu());
    });
}

fn without_a_terminal_the_client_runs_directly(t: &mut Ctx) {
    both(t, |fx| {
        let r = fx.run_helper(&["remote-host", "claude", "k", "n", "--", "/bin/sh", "-c", "echo direct; exit 3"]);
        assert_eq!((r.status.py(), r.out().as_str(), r.err().as_str()), (3, "direct\n", ""));
        assert_eq!(fx.files(), NO_FILES);
        // 找不到客户端：和 env 一样 127
        let r = fx.run_helper(&["remote-host", "claude", "k", "n", "--", "/nonexistent/client"]);
        assert_eq!(r.status.py(), 127);
        assert!(r.err().contains("/nonexistent/client"), "{}", r.err());
    });
}

fn terminal_gone_session_goes_on_and_can_be_attached(t: &mut Ctx) {
    both(t, |fx| {
        let mut h = fx.host(&[]);
        h.expect("<ready");
        h.send("say before\r");
        h.expect("<before>");
        let listed = fx.run_helper(&["remote-list"]).out();
        assert!(listed.contains("启动它的终端开着"), "{listed}");
        h.hangup();
        wait_for("列表里说没有终端接着", Duration::from_secs(5), || {
            if fx.run_helper(&["remote-list"]).out().contains("没有终端接着") {
                return true;
            }
            assert!(h.alive(), "终端没了宿主不能退出");
            false
        });
        let row = fx.run_helper(&["remote-list"]).out();
        let sid8 = row.split_whitespace().next().unwrap();
        assert!(matches(r"^[0-9a-f]{8}  claude  工作  ~/项目 one  \d\d-\d\d \d\d:\d\d 开始  没有终端接着\n$", &row), "{row:?}");
        // 接进去：先看到之前的输出（回放），再能操作
        let mut a = fx.attach_sized(&[], 20, 60);
        a.expect("<before>");
        a.expect("<size 20 60>"); // 只有它一个：照它的大小
        a.send("say again\r");
        a.expect("<again>");
        let listed = fx.run_helper(&["remote-list"]).out();
        assert!(listed.contains("1 处接入"), "{listed}");
        // 断开：Ctrl-\ 再按 d。会话还在
        a.send(b"\x1cd");
        assert_eq!(a.wait(), 0);
        let said = a.text();
        assert!(said.contains(&format!("已断开，会话还在运行（再接入：cca remote attach {sid8}）")), "{said:?}");
        assert!(h.alive());
        // 按编号的前缀再接入；会话结束时接入端的退出码是客户端的
        let mut b = fx.attach(&[&sid8[..5]]);
        b.expect("<again>");
        b.send("exit 3\r");
        assert_eq!(b.wait(), 3);
        let said = b.text();
        assert!(said.contains("会话已经结束（退出码 3）"), "{said:?}");
        assert_eq!(h.wait(), 3);
        assert_eq!(fx.files(), NO_FILES);
    });
}

fn two_terminals_share_one_session(t: &mut Ctx) {
    both(t, |fx| {
        let mut h = fx.host(&[]);
        h.expect("<ready");
        let mut a = fx.attach_sized(&[], 40, 60);
        a.expect("<ready"); // 回放
        h.expect("<size 24 60>"); // 两边都画得下的大小
        let (mut mark_h, mut mark_a) = (h.mark(), a.mark());
        // 查询只发给最近有输入的那个终端：一开始是启动的终端
        h.send("query\r");
        h.expect_since("<asked>", mark_h);
        a.expect_since("<asked>", mark_a);
        assert!(has(&h.out()[mark_h..], b"\x1b[6n"));
        assert!(has(&h.out()[mark_h..], b"\x1b]11;?\x07"));
        assert!(!has(&a.out()[mark_a..], b"\x1b[6n"));
        assert!(!has(&a.out()[mark_a..], b"\x1b]11;?"));
        // 接入端敲了键以后是它
        (mark_h, mark_a) = (h.mark(), a.mark());
        a.send("query\r");
        a.expect_since("<asked>", mark_a);
        h.expect_since("<asked>", mark_h);
        assert!(has(&a.out()[mark_a..], b"\x1b[6n"));
        assert!(!has(&h.out()[mark_h..], b"\x1b[6n"));
        // 只看的接入：按键不过去
        let mut r = fx.attach_sized(&["-r"], 50, 120);
        r.expect("<ready");
        mark_h = h.mark();
        r.send("say from-reader\r");
        h.send("say from-host\r");
        h.expect_since("<from-host>", mark_h);
        assert!(!has(&h.out()[mark_h..], b"from-reader"));
        let listed = fx.run_helper(&["remote-list"]).out();
        assert!(listed.contains("另有 2 处接入"), "{listed}");
        // 小的那个走了，大小回到启动的终端的
        mark_h = h.mark();
        a.send(b"\x1c.");
        assert_eq!(a.wait(), 0);
        h.expect_since("<size 24 80>", mark_h);
        h.send("exit 0\r");
        assert_eq!(h.wait(), 0);
        assert_eq!(r.wait(), 0);
        let said = r.text();
        assert!(said.contains("会话已经结束"), "{said:?}");
    });
}

fn only_the_primary_terminal_gets_what_is_not_plainly_safe(t: &mut Ctx) {
    both(t, |fx| {
        let mut h = fx.host(&[]);
        h.expect("<ready");
        let mut a = fx.attach(&[]);
        a.expect("<ready");
        let (mut mark_h, mut mark_a) = (h.mark(), a.mark());
        h.send("evil\r");
        h.expect_since("<done>", mark_h);
        a.expect_since("<done>", mark_a);
        // 主的那个（启动的终端）收到的和客户端写的逐字节一样
        let written = [&b"<evil>\x1b\x1b[c]52;c;ZXZpbA==\x07\x1b["[..], &b"0;".repeat(300), b"c<done>"].concat();
        assert!(has(&h.out()[mark_h..], &written));
        // 另一个：残缺的 ESC、查询、超长的序列都没有；那截字只是字，前面没有 ESC
        let got = &a.out()[mark_a..];
        assert!(has(got, b"<evil>]52;c;ZXZpbA==\x07<done>"));
        assert!(!has(got, b"\x1b]52"));
        assert!(!has(got, b"\x1b[c"));
        // 写剪贴板只给主的那个
        (mark_h, mark_a) = (h.mark(), a.mark());
        h.send("clip\r");
        h.expect_since("<clipped>", mark_h);
        a.expect_since("<clipped>", mark_a);
        assert!(has(&h.out()[mark_h..], b"\x1b]52;c;Y2xpcA==\x07"));
        assert!(!has(&a.out()[mark_a..], b"52;c;Y2xpcA"));
        // 后接入的：回放里也没有
        let mut late = fx.attach(&[]);
        late.expect("<clipped>");
        assert!(!has(late.out(), b"\x1b]52"));
        assert!(!has(late.out(), b"Y2xpcA"));
        assert!(!has(late.out(), b"\x1b[c"));
        assert!(has(late.out(), b"<evil>]52;c;ZXZpbA==<done>"), "回放里那截字还在（响铃去掉了）");
        h.send("exit 0\r");
        assert_eq!(h.wait(), 0);
    });
}

fn late_viewer_gets_the_modes_and_puts_its_terminal_back(t: &mut Ctx) {
    both(t, |fx| {
        let mut h = fx.host(&[]);
        h.expect("<ready");
        h.send("alt\r");
        h.expect("<alt>");
        // 把打开模式的那几条序列挤出回放：后接入的照样要先进备用屏幕
        h.send("flood\r");
        h.expect_in("<flooded>", Duration::from_secs(30), 0);
        let mut a = fx.attach(&[]);
        let end = a.expect("<flooded>");
        let first = &a.out()[..a.out().len().min(200)];
        assert!(first.starts_with(b"\x1b[?1049h"), "{:?}", String::from_utf8_lossy(first));
        assert!(has(first, b"\x1b[?25l"));
        assert!(has(first, b"\x1b[?2004h"));
        assert!(!has(&a.out()[..end], b"<alt>"), "回放只留最近的");
        assert!(end < 400 * 1024, "{end}");
        a.send(b"\x1cd");
        assert_eq!(a.wait(), 0);
        let tail = &a.out()[end..];
        for seq in [&b"\x1b[?25h"[..], b"\x1b[?2004l", b"\x1b[0m", b"\x1b[?1049l"] {
            assert!(has(tail, seq), "{:?}", String::from_utf8_lossy(seq));
        }
        assert!(term::find(tail, b"\x1b[?2004l").unwrap() < term::find(tail, b"\x1b[?1049l").unwrap(), "最后才离开备用屏幕");
        h.send("exit 0\r");
        assert_eq!(h.wait(), 0);
    });
}

fn attach_from_a_terminal_the_app_owns(t: &mut Ctx) {
    both(t, |fx| {
        let mut h = fx.host(&[]);
        h.expect("<ready 24 80");
        // 界面 App 那样起的终端：只给环境，不换目录
        let mut cmd = Cmd::new(harness::helper());
        cmd.arg("remote-attach").envs(fx.env.iter().map(|(k, v)| (k, v)));
        let mut v = Term::start_app(&mut cmd, 24, 80);
        v.expect("<ready 24 80");
        // 接入端把这个终端认成了控制终端：窗口大小变了它收得到信号、告诉宿主（不认的话会话的大小永远不跟着变）
        v.resize(20, 70);
        h.expect("<size 20 70>");
        v.send("say typed-in-the-app\n");
        h.expect("<typed-in-the-app>");
        v.expect("<typed-in-the-app>");
        // App 的「断开」是把终端关掉：接入端断开（退出码 0），会话接着运行、大小回到启动它的终端的
        v.hangup();
        assert_eq!(v.wait(), 0);
        h.expect("<size 24 80>");
        h.send("say still-running\n");
        h.expect("<still-running>");
        h.send("exit 0\n");
        assert_eq!(h.wait(), 0);
    });
}

fn attach_errors(t: &mut Ctx) {
    both(t, |fx| {
        let r = fx.run_helper(&["remote-attach"]);
        assert_eq!((r.status.py(), r.out().as_str()), (65, ""));
        assert!(r.err().contains("现在没有可以接入的会话"), "{}", r.err());
        assert_eq!(fx.run_helper(&["remote-attach", "not-hex"]).status.py(), 64);
        let mut h1 = fx.host(&[]);
        h1.expect("<ready");
        let mut h2 = fx.host(&[]);
        h2.expect("<ready");
        let listed = fx.run_helper(&["remote-list"]).out();
        let rows: Vec<&str> = listed.lines().collect();
        assert_eq!(rows.len(), 2, "{listed}");
        let first = rows[0].split_whitespace().next().unwrap();
        // 两个会话：要说接入哪个
        let r = fx.run_helper(&["remote-attach"]);
        assert_eq!(r.status.py(), 64);
        assert!(r.err().contains("有 2 个会话"), "{}", r.err());
        assert_eq!(fx.run_helper(&["remote-attach", "ffffffffffff0000"]).status.py(), 65);
        // 不在终端里：不接
        let r = fx.run_helper(&["remote-attach", first]);
        assert_eq!(r.status.py(), 69);
        assert!(r.err().contains("要在终端里运行"), "{}", r.err());
        // 宿主被强行结束：接入端说连接断了；它留下的文件下一次列表时清掉
        let mut a = fx.attach(&[first]);
        a.expect("<ready");
        let of_h1 = json::dumps(&Value::List(fx.metas(h1.pid())), &Style::default());
        let target = if of_h1.contains(first) { &mut h1 } else { &mut h2 };
        target.signal(libc::SIGKILL);
        assert_eq!(a.wait(), 68);
        let said = a.text();
        assert!(said.contains("连接断了"), "{said:?}");
        target.wait();
        assert_eq!(fx.files().len(), 4);
        assert_eq!(fx.run_helper(&["remote-list"]).out().lines().count(), 1);
        assert_eq!(fx.files().len(), 2);
    });
}

/// remote-end：等于关掉会话的终端。里面的程序收到 SIGHUP；不理的过几秒被强行结束
fn end_a_session(t: &mut Ctx) {
    both(t, |fx| {
        let r = fx.run_helper(&["remote-end"]);
        assert_eq!((r.status.py(), r.out().as_str()), (65, ""));
        assert!(r.err().contains("没有在会话宿主里运行的会话"), "{}", r.err());
        assert_eq!(fx.run_helper(&["remote-end", "not-hex"]).status.py(), 64);
        let mut h1 = fx.host(&[]);
        h1.expect("<ready");
        // 不理 SIGHUP 的客户端（忽略是连它起的进程一起的）。脚本后面多带的那个参数（它的 $0）只为了让这个进程的命令行里
        // 有临时目录：万一没收场，运行器找得到它
        let stubborn = "trap \"\" HUP; echo stubborn; while :; do sleep 1; done";
        let mut h2 = fx.term(&["remote-host", "codex", "k", "倔", "--", "/bin/sh", "-c", stubborn, fx.client.as_str()], 24, 80);
        h2.expect("stubborn");
        // 两个会话：要说结束哪个；--show 只找出是哪一个，不动它
        let r = fx.run_helper(&["remote-end"]);
        assert_eq!((r.status.py(), r.out().as_str()), (64, ""));
        assert!(r.err().contains("有 2 个会话"), "{}", r.err());
        assert_eq!(fx.run_helper(&["remote-end", "ffffffffffff0000"]).status.py(), 65);
        let (m1, m2) = (only(fx.metas(h1.pid())), only(fx.metas(h2.pid())));
        let r = fx.run_helper(&["remote-end", "--show", &m1["sid"].str()[..6].to_uppercase()]);
        assert_eq!((r.status.py(), r.err().as_str()), (0, ""));
        let shown = r.out();
        let fields: Vec<&str> = shown.trim_end_matches('\n').split('\t').collect();
        assert_eq!(fields.len(), 2, "{shown:?}");
        let (sid, row) = (fields[0], fields[1]);
        assert_eq!(sid, m1["sid"].str());
        assert!(matches(&format!(r"^{}  claude  工作  ~/项目 one  .* 开始  启动它的终端开着$", &sid[..8]), row), "{row:?}");
        assert!(h1.alive());
        // 接着的各处都收到“会话结束了”；客户端是被 SIGHUP 结束的（128 + 1），文件都清掉
        h1.hangup();
        let mut a = fx.attach(&[&sid[..8]]);
        a.expect("<ready");
        let r = fx.run_helper(&["remote-end", sid]);
        assert_eq!((r.status.py(), r.out().as_str(), r.err().as_str()), (0, "", ""));
        assert_eq!((a.wait(), h1.wait()), (129, 129));
        let said = a.text();
        assert!(said.contains("会话已经结束（退出码 129）"), "{said:?}");
        assert_eq!(fx.metas(h1.pid()), Vec::<Value>::new());
        // 不理 SIGHUP 的：过几秒被强行结束（128 + 9），命令等到它真的结束才回来
        let start = Instant::now();
        let r = fx.run_helper(&["remote-end", &m2["sid"].str()[..8]]);
        assert_eq!((r.status.py(), r.out().as_str(), r.err().as_str()), (0, "", ""));
        assert!(start.elapsed() > Duration::from_secs(4), "{:?}", start.elapsed());
        assert_eq!(h2.wait(), 137);
        assert_eq!(fx.files(), NO_FILES);
        assert_eq!(fx.run_helper(&["remote-end", sid]).status.py(), 65, "已经结束的：没有这个会话");
    });
}

fn a_host_that_does_not_answer_is_not_cleaned_up(t: &mut Ctx) {
    both(t, |fx| {
        let mut h = fx.host(&[]);
        h.expect("<ready");
        h.signal(libc::SIGSTOP); // 被挂起的宿主不回答：不能当它不在了、把它的 socket 删掉
        let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let row = fx.run_helper(&["remote-list"]).out();
            assert!(matches(r"^[0-9a-f]{8}  claude  工作  .* 开始  没有回应（可能被挂起了、正忙）\n$", &row), "{row:?}");
            assert_eq!(fx.files().len(), 2);
        }));
        h.signal(libc::SIGCONT);
        if let Err(failed) = checked {
            std::panic::resume_unwind(failed);
        }
        let listed = fx.run_helper(&["remote-list"]).out();
        assert!(listed.contains("启动它的终端开着"), "{listed}");
        h.send("exit 0\r");
        assert_eq!(h.wait(), 0);
        assert_eq!(fx.files(), NO_FILES);
    });
}

fn switch(t: &mut Ctx) {
    both(t, |fx| {
        let cfg = fx.cca.join("config.json");
        assert_eq!(fx.run_helper(&["remote-set", "on"]).status.py(), 0);
        assert_eq!(fsx::read_json(&cfg)["remote"], Value::Bool(true));
        assert_eq!(fx.run_helper(&["remote-set", "off"]).status.py(), 0);
        assert!(!fsx::read_json(&cfg).obj().contains_key("remote"));
        assert_eq!(fx.run_helper(&["remote-set", "maybe"]).status.py(), 1);
        assert_eq!(fx.run_helper(&["remote-list"]).out(), "");
    });
}
