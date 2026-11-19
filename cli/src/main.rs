//! cca-core：cca 的辅助程序，原来内嵌在 cca 里的 Python（PY_SRC）的 Rust 版。
//! bash 照旧 `py 子命令 参数…` 地调它：子命令的名字、参数、标准输入、输出和退出码都和原来一样。

#![allow(dead_code)] // 地基先行：各模块移植进来之前，很多函数还没有调用方

mod core;
mod py;

fn main() {
    std::process::exit(0);
}
