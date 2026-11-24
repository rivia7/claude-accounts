# 从 macOS 交叉编译 Linux 的两个目标（静态链接 musl）之前 source 这一份：
#   . cli/musl-env.sh && (cd cli && cargo build --release --locked --target x86_64-unknown-linux-musl)
# ring 里有 C 和汇编，要告诉它的构建脚本用哪个编译器、归档工具和开关（链接照旧用 Rust 自带的 rust-lld，见 .cargo/config.toml）：
#   - 编译器用系统的 clang（它什么目标都能编），归档用 rustup 的 llvm-tools 组件里的 llvm-ar（系统的 ar 不认 ELF；
#     rust-toolchain.toml 里写着这个组件，rustup 会自动装）
#   - 这台机器上没有 musl 的头文件：-ffreestanding 和 RING_CORE_NOSTDLIBINC 让 ring 的 C 代码不去找它们；
#     -U__musl__：苹果 clang 自带的 stddef.h 见到这个宏会转去找 musl 的那一份
# 在 Linux 上直接编（本机的 cc 就行）时什么都不做。出错时返回非 0、说明缺什么。
[ "$(uname -s)" = Darwin ] || return 0
_cca_cli=$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd) || return 1
_cca_ar=$(cd "$_cca_cli" && printf '%s/lib/rustlib/%s/bin/llvm-ar' "$(rustc --print sysroot)" "$(rustc -vV | sed -n 's/^host: //p')") || return 1
[ -x "$_cca_ar" ] || { echo "交叉编译 Linux 的目标要 llvm-ar：在 cli/ 里运行 rustup component add llvm-tools" >&2; unset _cca_cli _cca_ar; return 1; }
for _cca_t in x86_64_unknown_linux_musl aarch64_unknown_linux_musl; do
	export "CC_$_cca_t=clang" "AR_$_cca_t=$_cca_ar" "CFLAGS_$_cca_t=-U__musl__ -ffreestanding -DRING_CORE_NOSTDLIBINC=1"
done
unset _cca_cli _cca_ar _cca_t
