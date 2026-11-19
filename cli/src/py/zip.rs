//! `zipfile.ZipFile(路径)` 只读的那一半：目录表（`infolist()`）、读一个条目（`read()` / `open()`）。照 CPython 3.14 的
//! zipfile 逐段移植，接受什么、拒绝什么、出的是哪一类异常都照它（tests/vectors/clients.json 的 zip 一节，
//! tools/gen_clients_vectors.py 生成）。客户端更新解压官方安装包用它。
//!
//! 出错的类别（`ZipError`）和 Python 的异常类一一对应：调用方靠 `except (zipfile.BadZipFile, UnicodeError, OSError, RuntimeError,
//! ValueError)` 这样的名单分情况，名单以外的（`zlib.error`、`EOFError`）在原来是没人接的。
//!
//! 和 Python 不一样的地方：
//! - 压缩方法只有不压缩和 deflate。bzip2、lzma、zstd 的条目原来读得了（装了对应模块的话），这里是 `NotImplemented`
//! - deflate 用 miniz_oxide。好的数据解出来一样；坏的数据两边不一定在同一处、以同一种方式发现：zlib 说「数据不对」
//!   （`zlib.error`）的，这里个别情形会一直解到最后才在 CRC 上对不上（`BadZipFile`），反过来也可能（往回引用的距离超出已经
//!   解出来的范围、不完整的码表这一类）。哪一种都是读不了
//! - 目录表声称的长度大得要不到内存时原来是 MemoryError：多大算要不到随机器，这里固定从 2^47 字节算起（见 `read_at`）
//! - 两种警告不打印（原来写到 stderr）：文件名的 Unicode 附加字段是空的、两个条目指着同一处数据
//! - 文件名里落单的代理项（UTF-8 写不出来）不会有：不是合法 UTF-8 的名字两边都是读不了

#[path = "zip_cp437.rs"]
mod cp437;

use miniz_oxide::inflate::stream::{inflate, InflateState};
use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;

pub const ZIP_STORED: u16 = 0;
pub const ZIP_DEFLATED: u16 = 8;
const MAX_EXTRACT_VERSION: u8 = 63;
const ZIP_MAX_COMMENT: u64 = (1 << 16) - 1;
const SIZE_END_CENT_DIR: u64 = 22;
const SIZE_END_CENT_DIR64: u64 = 56;
const SIZE_END_CENT_DIR64_LOCATOR: u64 = 20;
const SIZE_CENTRAL_DIR: usize = 46;
const SIZE_FILE_HEADER: usize = 30;
/// `ZipExtFile.MAX_N`（`1 << 31 - 1`：减号先算，就是 2^30）
const MAX_N: u64 = 1 << 30;
const MIN_READ_SIZE: u64 = 4096;

#[derive(Debug)]
pub enum ZipError {
    /// `zipfile.BadZipFile`
    Bad(String),
    /// OSError
    Os(io::Error),
    /// NotImplementedError（RuntimeError 的子类）
    NotImplemented(String),
    /// RuntimeError
    Runtime(String),
    /// `zlib.error`
    Zlib(String),
    /// EOFError：条目的数据没到声称的长度，文件就到头了
    Eof,
    /// UnicodeDecodeError（ValueError 的子类）：标了 UTF-8 的文件名不是 UTF-8
    Unicode(String),
    /// OverflowError：要读的长度放不进 C 的整数
    Overflow(String),
    /// MemoryError：要读的长度大得要不到内存
    Memory,
    /// ValueError：位置放不进 64 位（`cannot fit 'int' into an offset-sized integer`）
    Value(String),
}

impl From<io::Error> for ZipError {
    fn from(e: io::Error) -> ZipError {
        ZipError::Os(e)
    }
}

fn bad<T>(msg: impl Into<String>) -> Result<T, ZipError> {
    Err(ZipError::Bad(msg.into()))
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().expect("8 个字节"))
}

/// `binascii.crc32(data, crc)`
pub fn crc32(data: &[u8], crc: u32) -> u32 {
    static TABLE: std::sync::LazyLock<[u32; 256]> = std::sync::LazyLock::new(|| {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        table
    });
    let mut c = !crc;
    for b in data {
        c = TABLE[((c ^ *b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// 目录表里的一项（`zipfile.ZipInfo`）
#[derive(Clone, Debug)]
pub struct ZipInfo {
    /// 压缩包里原样的名字（解码以后）
    pub orig_filename: String,
    /// 截到第一个 \0 为止的名字（有 Unicode 附加字段时是它给的）
    pub filename: String,
    pub flag_bits: u16,
    pub compress_type: u16,
    pub crc: u32,
    pub compress_size: u64,
    pub file_size: u64,
    /// 本地文件头在哪（加上了压缩包前面多出来的那一段）。原来是 Python 的整数，可以是负的、可以很大
    pub header_offset: i128,
    pub external_attr: u32,
    /// 哪种系统上打的包（`ZipInfo.create_system`；调试输出里看得到，别的地方不用）
    #[allow(dead_code)]
    pub create_system: u8,
    pub extract_version: u8,
    /// 下一个本地文件头（或者目录表）从哪开始：数据不能越过它
    end_offset: i128,
}

impl ZipInfo {
    /// `info.is_dir()`：名字以 / 结尾
    pub fn is_dir(&self) -> bool {
        self.filename.ends_with('/')
    }
}

/// `_sanitize_filename`：截到第一个 \0
fn sanitize(name: &str) -> String {
    match name.find('\0') {
        Some(i) => name[..i].to_string(),
        None => name.to_string(),
    }
}

/// 文件名解码：标了 UTF-8 的照 UTF-8（不对是 UnicodeDecodeError），别的照 cp437
fn decode_name(raw: &[u8], utf8: bool) -> Result<String, ZipError> {
    if utf8 {
        return match std::str::from_utf8(raw) {
            Ok(s) => Ok(s.to_string()),
            Err(e) => Err(ZipError::Unicode(crate::py::json::utf8_error_text(raw, &e))),
        };
    }
    Ok(raw.iter().map(|&b| if b < 0x80 { b as char } else { cp437::CP437_HIGH[(b - 0x80) as usize] }).collect())
}

/// 一个打开的压缩包（`zipfile.ZipFile(path)`）
pub struct ZipFile {
    file: File,
    infos: Vec<ZipInfo>,
}

/// `f.seek(pos); f.read(n)`：读到 n 个字节或者文件到头。照 CPython 3.14 实测：pos 放不进 64 位是 ValueError，是负的是 OSError；
/// n 到了 2^63 - 1 是 OverflowError，再往下大得要不到内存是 MemoryError（实测 2^40 还要得到、2^47 要不到，中间在哪断随机器，
/// 这里从 2^47 算起），别的就是读到头为止
fn read_at(file: &File, pos: i128, n: u64) -> Result<Vec<u8>, ZipError> {
    if pos > i64::MAX as i128 || pos < i64::MIN as i128 {
        return Err(ZipError::Value("cannot fit 'int' into an offset-sized integer".into()));
    }
    if pos < 0 {
        return Err(ZipError::Os(io::Error::from_raw_os_error(libc::EINVAL)));
    }
    if n >= i64::MAX as u64 {
        return Err(ZipError::Overflow("cannot fit 'int' into an index-sized integer".into()));
    }
    if n >= 1 << 47 {
        return Err(ZipError::Memory);
    }
    let size = file.metadata()?.len();
    let pos = pos as u64;
    let n = n.min(size.saturating_sub(pos)) as usize;
    let mut buf = vec![0u8; n];
    let mut got = 0;
    while got < n {
        match file.read_at(&mut buf[got..], pos + got as u64) {
            Ok(0) => break,
            Ok(k) => got += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    buf.truncate(got);
    Ok(buf)
}

/// 「目录表结束」记录里要的几样
struct EndRec {
    zip64: bool,
    /// 目录表多长、从哪开始（压缩包里写的）
    size: u64,
    offset: u64,
    /// 这条记录自己在文件里的位置
    location: u64,
}

/// `_EndRecData`：找「目录表结束」记录。没有是 None
fn end_rec_data(file: &File, filesize: u64) -> Result<Option<EndRec>, ZipError> {
    if filesize < SIZE_END_CENT_DIR {
        return Ok(None); // fpin.seek(-22, 2) 失败
    }
    let data = read_at(file, (filesize - SIZE_END_CENT_DIR) as i128, SIZE_END_CENT_DIR)?;
    if data.len() == SIZE_END_CENT_DIR as usize && &data[0..4] == b"PK\x05\x06" && data[20..22] == [0, 0] {
        // 签名对、没有注释
        let rec = EndRec {
            zip64: false,
            size: u32_at(&data, 12) as u64,
            offset: u32_at(&data, 16) as u64,
            location: filesize - SIZE_END_CENT_DIR,
        };
        return end_rec_data64(file, filesize - SIZE_END_CENT_DIR, rec).map(Some);
    }
    // 要么不是 zip，要么带着注释：在文件末尾找记录的签名（注释最长 64K）
    let max_comment_start = filesize.saturating_sub(ZIP_MAX_COMMENT + SIZE_END_CENT_DIR);
    let data = read_at(file, max_comment_start as i128, ZIP_MAX_COMMENT + SIZE_END_CENT_DIR)?;
    let Some(start) = data.windows(4).rposition(|w| w == b"PK\x05\x06") else {
        return Ok(None);
    };
    let rec = &data[start..data.len().min(start + SIZE_END_CENT_DIR as usize)];
    if rec.len() != SIZE_END_CENT_DIR as usize {
        return Ok(None); // 记录不全
    }
    let location = max_comment_start + start as u64;
    let rec = EndRec { zip64: false, size: u32_at(rec, 12) as u64, offset: u32_at(rec, 16) as u64, location };
    end_rec_data64(file, location, rec).map(Some)
}

/// `_EndRecData64`：前面有 zip64 的那两条记录就照它们的（at 是普通那条记录在文件里的位置）
fn end_rec_data64(file: &File, at: u64, mut rec: EndRec) -> Result<EndRec, ZipError> {
    if at < SIZE_END_CENT_DIR64_LOCATOR {
        return Ok(rec); // 文件放不下 zip64 的记录
    }
    let data = read_at(file, (at - SIZE_END_CENT_DIR64_LOCATOR) as i128, SIZE_END_CENT_DIR64_LOCATOR)?;
    if data.len() != SIZE_END_CENT_DIR64_LOCATOR as usize || &data[0..4] != b"PK\x06\x07" {
        return Ok(rec);
    }
    let (diskno, disks) = (u32_at(&data, 4), u32_at(&data, 16));
    if diskno != 0 || disks > 1 {
        return bad("zipfiles that span multiple disks are not supported");
    }
    // 假定没有 zip64 extensible data。这一处的 seek 原来不在 try 里：位置是负的是 OSError，外面当作「不是 zip」
    let at64 = at as i128 - SIZE_END_CENT_DIR64_LOCATOR as i128 - SIZE_END_CENT_DIR64 as i128;
    let data = read_at(file, at64, SIZE_END_CENT_DIR64)?;
    if data.len() != SIZE_END_CENT_DIR64 as usize || &data[0..4] != b"PK\x06\x06" {
        return Ok(rec);
    }
    rec.zip64 = true;
    rec.size = u64_at(&data, 40);
    rec.offset = u64_at(&data, 48);
    Ok(rec)
}

impl ZipFile {
    /// `zipfile.ZipFile(path)`：打开、读目录表
    pub fn open(path: &str) -> Result<ZipFile, ZipError> {
        let file = File::open(path)?;
        let infos = Self::real_get_contents(&file)?;
        Ok(ZipFile { file, infos })
    }

    /// `z.infolist()`
    pub fn infolist(&self) -> &[ZipInfo] {
        &self.infos
    }

    /// `_RealGetContents`
    fn real_get_contents(file: &File) -> Result<Vec<ZipInfo>, ZipError> {
        let filesize = file.metadata()?.len();
        let endrec = match end_rec_data(file, filesize) {
            Ok(Some(rec)) => rec,
            Ok(None) | Err(ZipError::Os(_)) => return bad("File is not a zip file"),
            Err(e) => return Err(e),
        };
        let (size_cd, offset_cd) = (endrec.size, endrec.offset);
        // 压缩包前面接着别的东西时，各个位置都要挪这么多
        let mut concat = endrec.location as i128 - size_cd as i128 - offset_cd as i128;
        if endrec.zip64 {
            concat -= (SIZE_END_CENT_DIR64 + SIZE_END_CENT_DIR64_LOCATOR) as i128;
        }
        let start_dir = offset_cd as i128 + concat;
        if start_dir < 0 {
            return bad("Bad offset for central directory");
        }
        let data = read_at(file, start_dir, size_cd)?;
        let mut infos = Vec::new();
        let (mut total, mut at) = (0u64, 0usize);
        while total < size_cd {
            let Some(centdir) = data.get(at..at + SIZE_CENTRAL_DIR) else {
                return bad("Truncated central directory");
            };
            if &centdir[0..4] != b"PK\x01\x02" {
                return bad("Bad magic number for central directory");
            }
            at += SIZE_CENTRAL_DIR;
            let (name_len, extra_len, comment_len) =
                (u16_at(centdir, 28) as usize, u16_at(centdir, 30) as usize, u16_at(centdir, 32) as usize);
            // fp.read(n)：不够就有多少算多少
            let mut take = |n: usize| -> &[u8] {
                let piece = &data[at.min(data.len())..(at + n).min(data.len())];
                at += piece.len();
                piece
            };
            let raw_name = take(name_len);
            let name_crc = crc32(raw_name, 0);
            let flags = u16_at(centdir, 8);
            let orig_filename = decode_name(raw_name, flags & 0x800 != 0)?;
            let extra = take(extra_len).to_vec();
            take(comment_len);
            let mut info = ZipInfo {
                filename: sanitize(&orig_filename),
                orig_filename,
                flag_bits: flags,
                compress_type: u16_at(centdir, 10),
                crc: u32_at(centdir, 16),
                compress_size: u32_at(centdir, 20) as u64,
                file_size: u32_at(centdir, 24) as u64,
                header_offset: u32_at(centdir, 42) as i128,
                external_attr: u32_at(centdir, 38),
                create_system: centdir[5],
                extract_version: centdir[6],
                end_offset: 0,
            };
            if info.extract_version > MAX_EXTRACT_VERSION {
                return Err(ZipError::NotImplemented(format!("zip file version {:.1}", info.extract_version as f64 / 10.0)));
            }
            decode_extra(&mut info, &extra, name_crc)?;
            info.header_offset += concat;
            infos.push(info);
            total += (SIZE_CENTRAL_DIR + name_len + extra_len + comment_len) as u64;
        }
        // 每一项的数据到哪为止：按本地文件头的位置排，后一项的开头就是前一项的尽头（最后一项到目录表）
        let mut order: Vec<usize> = (0..infos.len()).collect();
        order.sort_by_key(|&i| infos[i].header_offset);
        let mut end_offset = start_dir;
        for &i in order.iter().rev() {
            infos[i].end_offset = end_offset;
            end_offset = infos[i].header_offset;
        }
        Ok(infos)
    }

    /// `z.open(info)`：核对本地文件头，返回读这个条目的东西
    pub fn open_member<'a>(&'a self, info: &'a ZipInfo) -> Result<ZipExtFile<'a>, ZipError> {
        let fheader = read_at(&self.file, info.header_offset, SIZE_FILE_HEADER as u64)?;
        if fheader.len() != SIZE_FILE_HEADER {
            return bad("Truncated file header");
        }
        if &fheader[0..4] != b"PK\x03\x04" {
            return bad("Bad magic number for file header");
        }
        let (name_len, extra_len) = (u16_at(&fheader, 26) as u64, u16_at(&fheader, 28) as u64);
        let fname = read_at(&self.file, info.header_offset + SIZE_FILE_HEADER as i128, name_len)?;
        // 读了多少就往后走多少（名字被文件的尽头截短时也是）
        let data_start = info.header_offset + SIZE_FILE_HEADER as i128 + fname.len() as i128 + extra_len as i128;
        if info.flag_bits & 0x20 != 0 {
            return Err(ZipError::NotImplemented("compressed patched data (flag bit 5)".into()));
        }
        if info.flag_bits & 0x40 != 0 {
            return Err(ZipError::NotImplemented("strong encryption (flag bit 6)".into()));
        }
        let fname_str = decode_name(&fname, u16_at(&fheader, 6) & 0x800 != 0)?;
        if fname_str != info.orig_filename {
            return bad(format!(
                "File name in directory {} and header {} differ.",
                crate::py::obj::repr_str(&info.orig_filename),
                crate::py::obj::repr_str(&fname_str)
            ));
        }
        if data_start + info.compress_size as i128 > info.end_offset && info.end_offset != info.header_offset {
            // 正好和下一项指着同一处的原来只是警告（接着读）
            return bad(format!("Overlapped entries: {} (possible zip bomb)", crate::py::obj::repr_str(&info.orig_filename)));
        }
        if info.flag_bits & 1 != 0 {
            return Err(ZipError::Runtime(format!(
                "File {} is encrypted, password required for extraction",
                crate::py::obj::repr_str(&info.filename)
            )));
        }
        let inflater = match info.compress_type {
            ZIP_STORED => None,
            ZIP_DEFLATED => Some(Inflater { state: InflateState::new_boxed(DataFormat::Raw), eof: false }),
            12 | 14 | 93 => return Err(ZipError::NotImplemented(format!("compression type {}", info.compress_type))),
            _ => return Err(ZipError::NotImplemented("That compression method is not supported".into())),
        };
        Ok(ZipExtFile {
            file: &self.file,
            pos: data_start,
            compress_left: info.compress_size,
            left: info.file_size,
            inflater,
            tail: Vec::new(),
            eof: false,
            readbuffer: Vec::new(),
            offset: 0,
            expected_crc: info.crc,
            running_crc: 0,
            name: &info.filename,
        })
    }

    /// `z.read(info)`：整个条目的内容
    pub fn read(&self, info: &ZipInfo) -> Result<Vec<u8>, ZipError> {
        self.open_member(info)?.read_all()
    }
}

/// `ZipInfo._decodeExtra`：附加字段里的 zip64 长度、Unicode 文件名
fn decode_extra(info: &mut ZipInfo, extra: &[u8], filename_crc: u32) -> Result<(), ZipError> {
    let mut extra = extra;
    while extra.len() >= 4 {
        let (tp, ln) = (u16_at(extra, 0), u16_at(extra, 2) as usize);
        if ln + 4 > extra.len() {
            return bad(format!("Corrupt extra field {tp:04x} (size={ln})"));
        }
        let mut data = &extra[4..ln + 4];
        if tp == 0x0001 {
            // ZIP64：大文件、大压缩包
            let mut next = |field: &str| -> Result<u64, ZipError> {
                if data.len() < 8 {
                    return bad(format!("Corrupt zip64 extra field. {field} not found."));
                }
                let v = u64_at(data, 0);
                data = &data[8..];
                Ok(v)
            };
            if info.file_size == 0xffff_ffff_ffff_ffff || info.file_size == 0xffff_ffff {
                info.file_size = next("File size")?;
            }
            if info.compress_size == 0xffff_ffff {
                info.compress_size = next("Compress size")?;
            }
            if info.header_offset == 0xffff_ffff {
                info.header_offset = next("Header offset")? as i128;
            }
        } else if tp == 0x7075 {
            // Unicode 的文件名
            if data.len() < 5 {
                return bad("Corrupt unicode path extra field (0x7075)");
            }
            if data[0] == 1 && u32_at(data, 1) == filename_crc {
                match std::str::from_utf8(&data[5..]) {
                    Ok(name) if !name.is_empty() => info.filename = sanitize(name),
                    Ok(_) => {} // 空的：原来只是警告
                    Err(_) => return bad("Corrupt unicode path extra field (0x7075): invalid utf-8 bytes"),
                }
            }
        }
        extra = &extra[ln + 4..];
    }
    Ok(())
}

/// `zlib.decompressobj(-15)`
struct Inflater {
    state: Box<InflateState>,
    eof: bool,
}

impl Inflater {
    /// `z.decompress(data, max_length)`：最多解出 max_length 个字节，返回用掉了多少输入（剩下的是 unconsumed_tail）。
    /// 解出来的只留前 keep 个字节（调用方本来就只要这么多：多的照解、不存，免得一个小条目撑出一大块内存）
    fn decompress(&mut self, input: &[u8], max_length: u64, keep: u64, out: &mut Vec<u8>) -> Result<usize, ZipError> {
        let mut buf = vec![0u8; 64 * 1024];
        let (mut consumed, mut produced) = (0usize, 0u64);
        while !self.eof && produced < max_length {
            let room = (max_length - produced).min(buf.len() as u64) as usize;
            let r = inflate(&mut self.state, &input[consumed..], &mut buf[..room], MZFlush::None);
            consumed += r.bytes_consumed;
            if (out.len() as u64) < keep {
                let n = r.bytes_written.min((keep - out.len() as u64) as usize);
                out.extend_from_slice(&buf[..n]);
            }
            produced += r.bytes_written as u64;
            match r.status {
                Ok(MZStatus::StreamEnd) => self.eof = true,
                Ok(_) => {
                    if r.bytes_consumed == 0 && r.bytes_written == 0 {
                        break;
                    }
                }
                Err(MZError::Buf) => break, // 输入用完了，等更多的
                Err(_) => return Err(ZipError::Zlib("Error -3 while decompressing data".into())),
            }
        }
        Ok(consumed)
    }

    /// `z.flush()`：没有更多输入了，把还压在里面的输出交出来。这里出的错不报（和 zlib 模块一样）
    fn flush(&mut self, keep: u64, out: &mut Vec<u8>) {
        let mut buf = vec![0u8; 64 * 1024];
        while !self.eof {
            let r = inflate(&mut self.state, &[], &mut buf, MZFlush::None);
            if (out.len() as u64) < keep {
                let n = r.bytes_written.min((keep - out.len() as u64) as usize);
                out.extend_from_slice(&buf[..n]);
            }
            match r.status {
                Ok(MZStatus::StreamEnd) => self.eof = true,
                Ok(_) if r.bytes_written > 0 => {}
                _ => break,
            }
        }
    }
}

/// 读一个条目（`zipfile.ZipExtFile`）：`read(n)` 一段一段读，读到头时核对 CRC
pub struct ZipExtFile<'a> {
    file: &'a File,
    /// 压缩数据读到文件里的哪了
    pos: i128,
    compress_left: u64,
    left: u64,
    inflater: Option<Inflater>,
    /// deflate：给了解压器、它还没用掉的输入
    tail: Vec<u8>,
    eof: bool,
    readbuffer: Vec<u8>,
    offset: usize,
    expected_crc: u32,
    running_crc: u32,
    name: &'a str,
}

impl ZipExtFile<'_> {
    /// `f.read(n)`（n 不小于 0）：读到 n 个字节或者条目到头
    pub fn read(&mut self, n: usize) -> Result<Vec<u8>, ZipError> {
        let end = n + self.offset;
        if end < self.readbuffer.len() {
            let buf = self.readbuffer[self.offset..end].to_vec();
            self.offset = end;
            return Ok(buf);
        }
        let mut n = end - self.readbuffer.len();
        let mut buf = self.readbuffer[self.offset..].to_vec();
        self.readbuffer.clear();
        self.offset = 0;
        while n > 0 && !self.eof {
            let data = self.read1(n as u64)?;
            if n < data.len() {
                buf.extend_from_slice(&data[..n]);
                self.readbuffer = data;
                self.offset = n;
                break;
            }
            n -= data.len();
            buf.extend_from_slice(&data);
        }
        Ok(buf)
    }

    /// `f.read()`：读到头
    pub fn read_all(&mut self) -> Result<Vec<u8>, ZipError> {
        let mut buf = self.readbuffer[self.offset..].to_vec();
        self.readbuffer.clear();
        self.offset = 0;
        while !self.eof {
            buf.extend(self.read1(MAX_N)?);
        }
        Ok(buf)
    }

    /// `_read2`：从文件里再读一段压缩数据
    fn read2(&mut self, n: u64) -> Result<Vec<u8>, ZipError> {
        if self.compress_left == 0 {
            return Ok(Vec::new());
        }
        let n = n.max(MIN_READ_SIZE).min(self.compress_left);
        let data = read_at(self.file, self.pos, n)?;
        self.pos += data.len() as i128;
        self.compress_left -= data.len() as u64;
        if data.is_empty() {
            return Err(ZipError::Eof);
        }
        Ok(data)
    }

    /// `_read1`：读一段、解压、算进 CRC
    fn read1(&mut self, n: u64) -> Result<Vec<u8>, ZipError> {
        if self.eof || n == 0 {
            return Ok(Vec::new());
        }
        let mut data;
        if self.inflater.is_some() {
            let mut input = std::mem::take(&mut self.tail);
            if n > input.len() as u64 {
                input.extend(self.read2(n - input.len() as u64)?);
            }
            data = Vec::new();
            let inflater = self.inflater.as_mut().expect("上面看过");
            let used = inflater.decompress(&input, n.max(MIN_READ_SIZE), self.left, &mut data)?;
            // 流结束以后多出来的输入不管（unused_data）；没结束的留到下次
            self.tail = if inflater.eof { Vec::new() } else { input[used..].to_vec() };
            self.eof = inflater.eof || (self.compress_left == 0 && self.tail.is_empty());
            if self.eof {
                inflater.flush(self.left, &mut data);
            }
        } else {
            data = self.read2(n)?;
            self.eof = self.compress_left == 0;
        }
        data.truncate(self.left.min(data.len() as u64) as usize);
        self.left -= data.len() as u64;
        if self.left == 0 {
            self.eof = true;
        }
        self.running_crc = crc32(&data, self.running_crc);
        if self.eof && self.running_crc != self.expected_crc {
            return bad(format!("Bad CRC-32 for file {}", crate::py::obj::repr_str(self.name)));
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_like_binascii() {
        assert_eq!(crc32(b"", 0), 0);
        assert_eq!(crc32(b"123456789", 0), 0xcbf4_3926);
        assert_eq!(crc32(b"6789", crc32(b"12345", 0)), 0xcbf4_3926);
    }

    /// 出错的类别照 Python 异常类的名字（`type(e).__name__`）
    fn class_of(e: &ZipError) -> &'static str {
        match e {
            ZipError::Bad(_) => "BadZipFile",
            ZipError::Os(_) => "OSError",
            ZipError::NotImplemented(_) => "NotImplementedError",
            ZipError::Runtime(_) => "RuntimeError",
            ZipError::Zlib(_) => "error",
            ZipError::Eof => "EOFError",
            ZipError::Unicode(_) => "UnicodeDecodeError",
            ZipError::Overflow(_) => "OverflowError",
            ZipError::Memory => "MemoryError",
            ZipError::Value(_) => "ValueError",
        }
    }

    /// 读一个条目的结果，写成对照数据里的样子：`ok:摘要:长度` 或者 `err:异常类`
    fn summary(r: Result<Vec<u8>, ZipError>) -> String {
        match r {
            Ok(data) => format!("ok:{}:{}", crate::py::hash::sha256_hex(&data), data.len()),
            Err(e) => format!("err:{}", class_of(&e)),
        }
    }

    fn wanted(v: &crate::py::obj::Dict) -> String {
        match v.get("err") {
            Some(class) => format!("err:{}", class.py_str()),
            None => format!("ok:{}:{}", v.at("ok").py_str(), v.at("len").py_str()),
        }
    }

    #[test]
    fn like_zipfile() {
        use crate::py::obj::V;
        // 右边是 CPython 3.14 的 zipfile 给的：打不开是哪一类异常；打得开就是每一项的属性和读出来的内容
        let doc = crate::py::json::loads(include_str!("../../tests/vectors/clients.json")).unwrap();
        let mut rows = doc.as_dict().unwrap().list("zip").unwrap().items();
        assert!(rows.len() > 200);
        rows.extend(crate::py::xml::test_extra("zip"));
        let dir = std::env::temp_dir().join(format!("cca-zip-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.zip");
        let (mut wrong, mut softer) = (Vec::new(), 0);
        for row in &rows {
            let row = row.as_list().unwrap();
            let (name, data, want) = (row.at(0).py_str(), crate::py::xml::test_blob(&row.at(1)), row.at(2).as_dict().unwrap());
            std::fs::write(&path, &data).unwrap();
            let z = match (ZipFile::open(path.to_str().unwrap()), want.get("err")) {
                (Err(e), Some(class)) if class.py_str() == class_of(&e) => continue,
                (Ok(z), None) => z,
                (got, _) => {
                    wrong.push(format!(
                        "{name}：打开时 python {:?}，rust {:?}",
                        want.get("err").map(|c| c.py_str()),
                        got.err().map(|e| format!("{e:?}"))
                    ));
                    continue;
                }
            };
            let entries = want.list("ok").unwrap().items();
            if entries.len() != z.infolist().len() {
                wrong.push(format!("{name}：python {} 项，rust {} 项", entries.len(), z.infolist().len()));
                continue;
            }
            for (info, want) in z.infolist().iter().zip(&entries) {
                let want = want.as_dict().unwrap();
                let got = crate::dict! {
                    "name" => info.filename.as_str(), "orig" => info.orig_filename.as_str(), "flags" => info.flag_bits as i64, "method" => info.compress_type as i64,
                    "crc" => info.crc as i64, "csize" => V::Big(info.compress_size.to_string().into()), "size" => V::Big(info.file_size.to_string().into()),
                    "attr" => info.external_attr as i64, "is_dir" => info.is_dir(),
                };
                for (k, v) in got.items() {
                    // 长度可能超过 i64：都写成文字比
                    if v.py_str() != want.at(&k).py_str() {
                        wrong.push(format!(
                            "{name} / {}：{k} python {}，rust {}",
                            info.filename,
                            want.at(&k).py_repr(),
                            v.py_repr()
                        ));
                    }
                }
                let read = want.dict("read").unwrap();
                if read.has("skip") {
                    continue;
                }
                let whole = summary(z.read(info));
                let chunks = summary(z.open_member(info).and_then(|mut f| {
                    let mut all = Vec::new();
                    loop {
                        let block = f.read(1024 * 1024)?;
                        if block.is_empty() {
                            return Ok(all);
                        }
                        all.extend(block);
                    }
                }));
                for (how, got, want) in [("read", whole, wanted(&read)), ("chunks", chunks, wanted(&want.dict("chunks").unwrap()))]
                {
                    if got == want {
                        continue;
                    }
                    // 有意不一样的两类（见文件头）：bzip2 / lzma / zstd 的条目这里不解；坏了的 deflate 数据两边都读不了、发现的方式可以不同
                    let failed = |s: &str| s.starts_with("err:");
                    let unsupported = matches!(info.compress_type, 12 | 14 | 93) && got == "err:NotImplementedError";
                    let both_fail = info.compress_type == ZIP_DEFLATED && failed(&got) && failed(&want);
                    if unsupported || both_fail {
                        softer += 1;
                    } else {
                        wrong.push(format!("{name} / {} {how}：python {want}，rust {got}", info.filename));
                    }
                }
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(wrong.is_empty(), "{} 处和 Python 不一样：\n{}", wrong.len(), wrong[..wrong.len().min(40)].join("\n"));
        assert!(softer < rows.len() / 8, "有意不一样的太多了：{softer}");
    }

    #[test]
    fn names() {
        assert_eq!(decode_name(b"a\x80\xff", false).unwrap(), "aÇ\u{a0}");
        assert_eq!(decode_name("中".as_bytes(), true).unwrap(), "中");
        assert!(matches!(decode_name(b"\xff", true), Err(ZipError::Unicode(m)) if m.contains("invalid start byte")));
        assert_eq!(sanitize("a\0b"), "a");
    }
}
