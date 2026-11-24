//! 切出来的 bash 配上替身来跑：几份 isolation_* 测试（tests/isolation_*.rs）共用的夹具。
//!
//! 这几份测试大多从 cca 里切出几个函数（[`crate::cca_src`]），前面接上一段替身函数（`fail_warn`、`py` 这些），后面接上要跑的
//! 那几行，交给 /bin/bash，再看退出码、两路输出和它留下的文件；系统命令（open、pgrep、lsof、curl……）换成 PATH 最前面的替身。
//! 原来是 `subprocess.run(['/bin/bash', '-c', 脚本], env=…, capture_output=True, timeout=…)`，或者把脚本从标准输入喂给 bash。
//! - [`c`]、[`piped`]：起 bash 的两种写法（脚本当 `-c` 的参数 / 从标准输入喂进去）。环境照 kit::proc 的规矩是空的，
//!   用到的一样样给，开发者 shell 里的东西漏不进去
//! - [`run`]：跑完拿回结果；起不来、过了时限就 panic（原来 subprocess.run 抛异常，用例算出错）
//! - [`double`]：写一个 `#!/bin/bash` 的替身
//! - [`guard`]、[`no_stray`]：只会报错的替身。被测的程序调到没准备假命令的外部命令时，替身把怎么被调的记下来；
//!   用例完了（失败了、超时了也一样）查这份记录是空的
//!
//! 替身里要算的（打出自己的环境、参数，搜正则，拿着文件不放……）交给 cca-test 的角色（tests/tool/double.rs），
//! cca-test 在哪经替身的环境变量给（原来给的是 `PYTHON`，替身里写 `"$PYTHON" -I -c '…'`）。

use crate::harness::Ctx;
use crate::proc::{Cmd, Output};
use std::path::{Path, PathBuf};

/// `/bin/bash -c 脚本`。环境是空的：要什么用 `env` 给
pub fn c(script: &str) -> Cmd {
    let mut cmd = Cmd::new("/bin/bash");
    cmd.arg("-c").arg(script);
    cmd
}

/// `/bin/bash`，脚本从标准输入喂进去（`subprocess.run(['/bin/bash'], input=脚本)`；喂完就关上）。
/// 只对 [`run`]（`Cmd::output`）有用：spawn 起来的自己往 `Child::stdin` 里写
pub fn piped(script: &str) -> Cmd {
    let mut cmd = Cmd::new("/bin/bash");
    cmd.input(script);
    cmd
}

/// 跑完，拿回退出码和两路输出。起不来、过了 cmd 设的时限：panic，说清楚是哪条命令、到那时为止说了什么
pub fn run(cmd: &mut Cmd) -> Output {
    cmd.output().unwrap_or_else(|e| panic!("{e}"))
}

/// 写一个替身：`bin/名字`，内容是 `#!/bin/bash` 一行加上 body，能执行（755）。返回它的路径
pub fn double(bin: &Path, name: &str, body: &str) -> PathBuf {
    let path = bin.join(name);
    crate::fsx::write_script(&path, &format!("#!/bin/bash\n{body}"));
    path
}

/// 只会报错的替身：把自己怎么被调的（`$0 参数…`）追加进 `$HOME/unexpected-commands.log`，退出码 127
pub const GUARD: &str = "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> \"$HOME/unexpected-commands.log\"\nexit 127\n";

/// 在 bin 里给这几个命令各放一个 [`GUARD`]：会动到这台机器、会联网的命令，被测的程序一个都不该调到
pub fn guard(bin: &Path, names: &[&str]) {
    for name in names {
        crate::fsx::write_script(bin.join(name), GUARD);
    }
}

/// 登记一件收尾的事（`addCleanup`）：用例完了查 `home/unexpected-commands.log`（[`GUARD`] 写的），有内容就算失败——
/// 调到了没有准备假命令的外部命令。用例自己失败了、超时了也照查（收尾跑在删临时目录之前）
pub fn no_stray(t: &mut Ctx, home: &Path) {
    let log = home.join("unexpected-commands.log");
    t.defer(move || {
        let said = if log.exists() { crate::fsx::read_text(&log) } else { String::new() };
        assert_eq!(said, "", "调到了没有准备假命令的外部命令");
    });
}
