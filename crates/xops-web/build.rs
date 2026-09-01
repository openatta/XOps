//! 把前端构建产物嵌进二进制。
//!
//! `D55`：**构建产物随二进制发行，部署方不需要 Node。** 所以不是"运行时去某个目录找"，
//! 是编译期把 `web/dist` 里的文件 `include_bytes!` 进来。
//!
//! `web/dist` 不存在时嵌一张空表——那是"只跑 API、不带页面"的形态，`cargo build`
//! 照样过。**它不该悄悄地过**，所以会打一条 warning。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("没有 CARGO_MANIFEST_DIR"));
    let dist = manifest.join("../../web/dist");
    println!("cargo:rerun-if-changed={}", dist.display());

    let mut entries = Vec::new();
    if dist.is_dir() {
        collect(&dist, &dist, &mut entries);
        entries.sort();
    } else {
        println!("cargo:warning=web/dist 不在，二进制里不带前端页面（跑 `npm run build` 生成它）");
    }

    let mut source = String::from(
        "/// 编译期嵌进来的前端产物：`(路径, 内容)`。\npub static EMBEDDED: &[(&str, &[u8])] = &[\n",
    );
    for (relative, absolute) in &entries {
        source.push_str(&format!(
            "    ({relative:?}, include_bytes!({absolute:?})),\n"
        ));
    }
    source.push_str("];\n");

    let out = PathBuf::from(env::var("OUT_DIR").expect("没有 OUT_DIR")).join("assets.rs");
    fs::write(&out, source).expect("写不出 assets.rs");
}

fn collect(root: &Path, directory: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push((
                relative.to_string_lossy().replace('\\', "/"),
                path.to_string_lossy().into_owned(),
            ));
        }
    }
}
