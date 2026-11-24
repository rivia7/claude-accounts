//! 各份测试合成的一个程序（cargo test --profile kit -p cca-tests）。一个模块就是 tests/ 下的一个 .rs：
//! 在这里加一行 `mod`、在表里加一项。怎么写用例、怎么跑、有哪些开关，见 kit::harness 开头的说明。

mod kit_selftest;

fn main() {
    kit::harness::main(env!("CARGO_BIN_EXE_cca-test"), &[kit_selftest::MODULE]);
}
