//! xtask/src/main.rs: [cli-entry] [version-helpers] [build-flow] [sub-builders]

mod zip_ext;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::{Parser, Subcommand};
use fs_extra::{dir, file};
use xshell::{Shell, cmd};
use zip::{CompressionMethod, write::FileOptions};

use crate::zip_ext::zip_create_from_directory_with_options;

// [cli-entry]

#[derive(Parser)]
#[command(name = "xtask", about = "ChiRi Build System")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 编译项目并打包
    #[command(alias = "b")]
    Build {
        /// 不打包 zip，仅组装模块目录（CI 使用，交由 GitHub 下载时自动打包）
        #[arg(long)]
        no_pack: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let sh = Shell::new()?;

    match cli.command {
        Commands::Build { no_pack } => build(&sh, no_pack)?,
    }

    Ok(())
}

// [version-helpers]

fn cal_git_code(sh: &Shell) -> Result<usize> {
    let output = cmd!(sh, "git rev-list --count HEAD").read()?;
    Ok(output.trim().parse::<usize>()?)
}

// fn get_time() -> String {
//     chrono::Local::now().format("%H%M").to_string()
// }

/// 从 module/module.prop（KEY=VALUE，Magisk/KernelSU 模块规范）读取 name 与 version，作为产物命名依据
fn read_module_prop() -> Result<(String, String)> {
    let content = fs::read_to_string("module/module.prop")?;
    let mut name = String::new();
    let mut version = String::new();
    for line in content.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "name" => name = value.trim().to_string(),
            "version" => version = value.trim().to_string(),
            _ => {}
        }
    }
    if name.is_empty() || version.is_empty() {
        anyhow::bail!("module/module.prop 缺少 name 或 version 字段");
    }
    Ok((name, version))
}

// [build-flow]

fn build(sh: &Shell, no_pack: bool) -> Result<()> {
    let temp_dir = temp_dir();

    // 产物命名以 module.prop 为准（name-version-提交数-时分）；无日期，同日多次构建靠时分区分、跨天靠版本号+提交数保证唯一
    let (module_name, module_version) = read_module_prop()?;
    let base_name = format!(
        "{}-{}-{}",
        module_name,
        module_version,
        cal_git_code(sh)?,
        // get_time()
    );

    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir)?;

    build_webui(sh)?;

    build_core(sh)?;

    let module_dir = Path::new("module").to_path_buf();
    dir::copy(
        &module_dir,
        &temp_dir,
        &dir::CopyOptions::new().overwrite(true).content_only(true),
    )?;

    if temp_dir.join(".gitignore").exists() {
        fs::remove_file(temp_dir.join(".gitignore"))?;
    }

    // 删模块文本文件中的整行 # 注释（保留 shebang、行内 # 与无扩展名标记文件）；
    // **只作用于 temp_dir 副本**：module/ 是唯一真身，任何构建中途失败/被杀都不会污染工作区
    // （旧实现由 CI 的 perl -i 或 build-arch.sh 的「换入副本 + trap 还原」完成，
    //  SIGKILL/断电时 trap 不执行会把 module/ 永久停在已剥状态）
    strip_comments(&temp_dir);

    // 从模块包移除仅二进制使用的配置：运行时只读嵌入内容，磁盘上无任何读取方，取消对外暴露以缩小可篡改面；meta.yaml / rules.yaml / 特调与 FAS 导出文件有 WebUI 读取方，保留
    const BIN_ONLY: [&str; 5] = [
        "config/feature.yaml",
        "config/normal/tuned_profiles.yaml",
        "config/normal/scenemode.yaml",
        "config/normal/fas.yaml",
        // 实验室模式定义：只给守护进程读，WebUI 不读（模式 key 与文案在 WebUI 侧硬编码）
        "config/rhine-init.yaml",
    ];
    for rel in BIN_ONLY {
        let _ = fs::remove_file(temp_dir.join(rel));
    }
    // normal/fas/ 目录（每应用 FAS 调优）整体只进二进制
    let _ = fs::remove_dir_all(temp_dir.join("config/normal/fas"));
    // 每 SoC 目录：feature.yaml 与 soc.yaml（硬件基线，仅作 sysfs 兜底基准）同样只进二进制。
    // 遍历到的 normal/i18n 子目录下无这两个文件，remove_file 失败忽略即可，无需特判目录名
    if let Ok(rd) = fs::read_dir(temp_dir.join("config")) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                let _ = fs::remove_file(e.path().join("feature.yaml"));
                let _ = fs::remove_file(e.path().join("soc.yaml"));
            }
        }
    }

    let bin_path = temp_dir.join("core").join("bin");
    fs::create_dir_all(&bin_path)?;

    file::copy(
        aarch64_bin_path(),
        bin_path.join("chiri"),
        &file::CopyOptions::new().overwrite(true),
    )?;

    let webroot_dir = temp_dir.join("webroot");
    dir::copy(
        Path::new("webui").join("dist"),
        &webroot_dir,
        &dir::CopyOptions::new().overwrite(true).content_only(true),
    )?;

    let output_dir = Path::new("output");
    fs::create_dir_all(output_dir)?;

    if no_pack {
        // 不打包：组装好的模块目录移出临时目录，交 CI/GitHub 打包，目录名即 artifact 名（下载时自动生成同名 .zip）
        let final_dir = output_dir.join(&base_name);
        let _ = fs::remove_dir_all(&final_dir);
        fs::rename(&temp_dir, &final_dir)?;
        println!("模块目录已生成: {}", final_dir.display());
    } else {
        let zip_path = output_dir.join(format!("{base_name}.zip"));
        println!("开始打包: {}", zip_path.display());

        let options: FileOptions<'_, ()> = FileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(9));

        zip_create_from_directory_with_options(&zip_path, &temp_dir, |_| options)?;
    }

    println!("构建成功！");
    Ok(())
}

// [sub-builders]

fn temp_dir() -> PathBuf {
    Path::new("output").join(".temp")
}

fn aarch64_bin_path() -> PathBuf {
    Path::new("target")
        .join("aarch64-linux-android")
        .join("release")
        .join("chiri")
}

fn build_core(sh: &Shell) -> Result<()> {
    println!("正在编译 Rust Core...");
    // push_env 会在当前作用域内设置环境变量，离开作用域自动恢复
    let _env = sh.push_env("RUSTFLAGS", "-C default-linker-libraries");
    cmd!(
        sh,
        "cargo +nightly ndk --platform 26 -t arm64-v8a build -Z build-std -r"
    )
    .run()?;
    Ok(())
}

fn build_webui(sh: &Shell) -> Result<()> {
    println!("正在编译 WebUI...");
    // push_dir 类似于 cd，离开作用域后会自动切回原目录
    let _dir = sh.push_dir("webui");
    cmd!(sh, "npm run build").run()?;
    Ok(())
}

// [strip] 在产物副本上删整行注释

/// 删模块文本文件中的整行 `#` 注释，保留 shebang（`#!`）、行内 `#`。
///
/// **只应在 `temp_dir` 副本上调用**：`module/` 是唯一真身，构建中断（含 SIGKILL/断电）
/// 不得污染它。谓词与历史 CI 的 `perl -i -ne 'print if /^\s*#!/ || !/^\s*#/'` 等价；
/// 排除 `META-INF/`（updater-script 的 `#MAGISK` 为必需标记）与 `scripts/`
/// （对外稳定接口，守护进程与 WebUI 共用），二者只在顶层生效。
fn strip_comments(root: &Path) {
    const EXTS: [&str; 5] = ["sh", "block", "yaml", "yml", "ftl"];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let top_level = path
                    .strip_prefix(root)
                    .map(|r| r.components().count() == 1)
                    .unwrap_or(false);
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if top_level && (name == "META-INF" || name == "scripts") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            let matched = path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|x| EXTS.contains(&x));
            if !matched {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let kept: String = text
                .split_inclusive('\n')
                .filter(|line| {
                    let t = line.trim_start();
                    t.starts_with("#!") || !t.starts_with('#')
                })
                .collect();
            if kept != text {
                let _ = fs::write(&path, kept);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与历史 CI 谓词等价的语义：整行 # 删；shebang、行内 #、META-INF/、scripts/ 保留
    #[test]
    fn strip_comments_semantics() {
        let root = Path::new("target").join("xtask-strip-test");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("META-INF")).unwrap();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::create_dir_all(root.join("foo")).unwrap();

        fs::write(root.join("a.sh"), "#!/bin/sh\n# comment\n  # indented\nx=1 # inline\n").unwrap();
        fs::write(root.join("foo/b.yaml"), "# c\nkey: val # inline\n").unwrap();
        fs::write(root.join("META-INF/keep.sh"), "#META\n").unwrap();
        fs::write(root.join("scripts/keep.sh"), "# c\nrun\n").unwrap();

        strip_comments(&root);

        assert_eq!(
            fs::read_to_string(root.join("a.sh")).unwrap(),
            "#!/bin/sh\nx=1 # inline\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("foo/b.yaml")).unwrap(),
            "key: val # inline\n"
        );
        // 顶层 META-INF/ 与 scripts/ 整体跳过
        assert_eq!(fs::read_to_string(root.join("META-INF/keep.sh")).unwrap(), "#META\n");
        assert_eq!(fs::read_to_string(root.join("scripts/keep.sh")).unwrap(), "# c\nrun\n");

        let _ = fs::remove_dir_all(&root);
    }

    /// 无尾随换行的末行 # 注释也要删干净（split_inclusive 边界）
    #[test]
    fn strip_comments_no_trailing_newline() {
        let root = Path::new("target").join("xtask-strip-test2");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("x.yml"), "a: 1\n# tail comment").unwrap();
        strip_comments(&root);
        assert_eq!(fs::read_to_string(root.join("x.yml")).unwrap(), "a: 1\n");
        let _ = fs::remove_dir_all(&root);
    }
}
