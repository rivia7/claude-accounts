//! exec：换会话、接上终端、换目录以后 exec 一个程序（测试起子进程时经它：测试程序是多线程的，不能自己 fork 了再做这些）

pub const CMDS: &[(&str, crate::Run)] = &[];
