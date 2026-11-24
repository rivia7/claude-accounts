//! gh-releases 目录：假 GitHub 的最近的发布（cca update --check）——按 目录/v<版本>/ 生成（里面要有 cca），测试版是 pre-release；
//! 更新说明取 v<版本>/notes（没有就是“cca 版本”）。写到 stdout，像 json.dump(…, ensure_ascii=False)

use super::claude::arg;
use super::*;
use kit::json::Style;

pub fn run(a: &[String]) -> R<i32> {
    let g = arg(a, 0)?;
    let mut names: Vec<String> = match std::fs::metadata(g) {
        Ok(m) if m.is_dir() => {
            std::fs::read_dir(g)?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()
        }
        _ => Vec::new(),
    };
    names.sort();
    let mut rel = Vec::new();
    for d in names {
        if !d.starts_with('v') || !std::fs::metadata(join(&[g, &d, "cca"])).is_ok_and(|m| m.is_file()) {
            continue;
        }
        let n = join(&[g, &d, "notes"]);
        let body = if exists(&n) { read_text(&n)? } else { format!("cca {}", &d[1..]) };
        rel.push(obj! {
            "tag_name" => d.as_str(),
            "name" => d.as_str(),
            "draft" => false,
            "prerelease" => d.contains("-beta."),
            "body" => body,
            "html_url" => format!("https://github.com/rivia7/claude-accounts/releases/tag/{d}"),
            "published_at" => "2026-11-01T08:00:00Z",
        });
    }
    write_out(&json::dumps(&Value::List(rel), &Style::default().unicode()));
    Ok(0)
}
