//! xtask/src/main.rs: [cli-entry] [lto] [version-helpers] [build-flow] [sub-builders]

mod zip_ext;

use std::{
    env, fs,
    io::{self, IsTerminal, Write},
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
        /// LTO 等级：full | thin | off | auto
        /// （auto：本地终端只询问一次；CI/非交互下各模块先 full，失败按模块回退 thin → off）
        #[arg(long, value_name = "LEVEL")]
        lto: Option<String>,
    },
    /// 把 output/ 下最新的模块目录打成 zip（本地构建脚本用；命名 `<目录名>-b<分秒>`）
    #[command(alias = "p")]
    Pack {
        /// 指定要打包的模块目录（缺省取 output/ 下最新修改的那个）
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let sh = Shell::new()?;

    match cli.command {
        Commands::Build { no_pack, lto } => build(&sh, no_pack, lto.as_deref())?,
        Commands::Pack { dir } => pack(dir)?,
    }

    Ok(())
}

// [lto]

/// LTO 等级（用户口径）。
///
/// Cargo/rustc 只接受 `fat` / `thin` / `off`：直接传 `full` 会报
/// `incorrect value `full` for codegen option `lto``。所以用户口中的 `full`
/// 在这里显式映射为 `fat`，CLI 与环境变量仍统一用 full / thin / off。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LtoLevel {
    Full,
    Thin,
    Off,
}

impl LtoLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Thin => "thin",
            Self::Off => "off",
        }
    }

    fn cargo_value(self) -> &'static str {
        match self {
            Self::Full => "fat",
            Self::Thin => "thin",
            Self::Off => "off",
        }
    }

    /// 按模块的回退阶梯：full → thin → off（None = 已到底）
    fn next(self) -> Option<Self> {
        match self {
            Self::Full => Some(Self::Thin),
            Self::Thin => Some(Self::Off),
            Self::Off => None,
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "full" | "fat" | "true" => Some(Self::Full),
            "thin" => Some(Self::Thin),
            "off" | "false" | "none" => Some(Self::Off),
            _ => None,
        }
    }
}

/// 本次构建的 LTO 方案
#[derive(Clone, Copy, Debug)]
enum LtoPlan {
    /// 固定等级（本地询问 / 显式 --lto / CHIRI_LTO）：全部模块共用，失败即失败
    Fixed(LtoLevel),
    /// CI 缺省：每个模块先 full，失败按模块回退 thin → off
    Auto,
}

const LTO_ENV: &str = "CHIRI_LTO";

/// 决定本次构建的 LTO 方案。
///
/// 优先级：`--lto` > `CHIRI_LTO` > auto。auto 时：
/// - 本地（stdin 是终端且无 CI 环境变量）→ 询问一次，全部模块共用；
/// - CI / 管道 / 重定向 → 不询问，按模块 full → thin → off 回退。
fn resolve_lto(cli_value: Option<&str>) -> Result<LtoPlan> {
    let raw = match cli_value {
        Some(value) => Some(value.to_string()),
        None => env::var(LTO_ENV).ok().filter(|v| !v.trim().is_empty()),
    };
    match raw.as_deref().map(str::trim) {
        None | Some("auto") => {
            if io::stdin().is_terminal() && env::var_os("CI").is_none() {
                Ok(LtoPlan::Fixed(ask_lto()?))
            } else {
                println!("LTO: auto（非交互：各模块先 full，失败按模块回退 thin → off）");
                Ok(LtoPlan::Auto)
            }
        }
        Some(value) => {
            let level = LtoLevel::parse(value).ok_or_else(|| {
                anyhow::anyhow!("无效 LTO 等级 `{value}`：只接受 full / thin / off / auto")
            })?;
            println!("LTO: {}（全部模块固定，不回退）", level.as_str());
            Ok(LtoPlan::Fixed(level))
        }
    }
}

/// 本地构建只问一次，答案对全部模块生效（chiri + yumi-ebpf）
fn ask_lto() -> Result<LtoLevel> {
    let mut stderr = io::stderr();
    write!(
        stderr,
        "\n选择 LTO 模式（本次构建全部模块共用）:\n\
         1) full —— 完全 LTO，产物最小/最快，构建最慢（缺省）\n\
         2) thin —— 增量 LTO，构建快一些，优化略弱\n\
         3) off  —— 关闭 LTO，构建最快，产物体积最大\n\
         选择 [1-3, 回车=1]: "
    )?;
    stderr.flush()?;

    loop {
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 {
            // 终端被关掉：按缺省走，不让构建无谓失败
            eprintln!("（stdin 已关闭，按缺省 full）");
            return Ok(LtoLevel::Full);
        }
        match line.trim() {
            "" | "1" | "full" | "fat" => return Ok(LtoLevel::Full),
            "2" | "thin" => return Ok(LtoLevel::Thin),
            "3" | "off" => return Ok(LtoLevel::Off),
            other => {
                eprintln!("无效选择 `{other}`，请输入 1 / 2 / 3");
                write!(stderr, "选择 [1-3, 回车=1]: ")?;
                stderr.flush()?;
            }
        }
    }
}

// [version-helpers]

fn cal_git_code(sh: &Shell) -> Result<usize> {
    let output = cmd!(sh, "git rev-list --count HEAD").read()?;
    Ok(output.trim().parse::<usize>()?)
}

// fn get_time() -> String {
//     chrono::Local::now().format("%H%M").to_string()
// }

/// 从 module/module.prop（KEY=VALUE）读取 name 与 version，作为产物命名依据
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

fn build(sh: &Shell, no_pack: bool, lto: Option<&str>) -> Result<()> {
    let temp_dir = temp_dir();
    // 先定 LTO 方案：本地询问要在耗时构建（webui）之前发生
    let lto_plan = resolve_lto(lto)?;

    // 产物命名以 module.prop 为准（name-version-提交数-时分），同日多次构建靠时分、跨天靠版本号+提交数区分
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

    build_core(sh, lto_plan)?;

    let module_dir = Path::new("module").to_path_buf();
    dir::copy(
        &module_dir,
        &temp_dir,
        &dir::CopyOptions::new().overwrite(true).content_only(true),
    )?;

    if temp_dir.join(".gitignore").exists() {
        fs::remove_file(temp_dir.join(".gitignore"))?;
    }

    // 删模块文本文件中的整行 # 注释（保留 shebang、行内 # 与无扩展名标记文件），只作用于 temp_dir 副本
    // module/ 是唯一真身，构建中途失败/被杀都不会污染工作区
    strip_comments(&temp_dir);

    // 从模块包移除仅二进制使用的配置：运行时只读嵌入内容，磁盘上无读取方；meta.yaml / rules.yaml / 特调与 FAS 导出文件有 WebUI 读取方，保留
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
    // i18n/*.ftl 同理：编译期由 include_dir!("module/config") 嵌入，运行期只读嵌入内容
    // （src/i18n.rs 明确「不读磁盘 i18n 目录」），装完在模块目录里没有读取方。
    // 只从包内剔除，仓库里的源文件必须保留：build.rs 的 assert_required_configs 要求它存在，
    // webui/scripts/fetch-fonts.mjs 也要读 zh.ftl 收集 CJK 字表做字体子集。
    let _ = fs::remove_dir_all(temp_dir.join("config/i18n"));
    // 每 SoC 目录的 feature.yaml 与 soc.yaml（硬件基线，仅作 sysfs 兜底基准）同样只进二进制
    // 遍历到的 normal/i18n 子目录下无这两个文件，remove_file 失败忽略即可
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
        // 不打包：模块目录移出临时目录，交 CI/GitHub 打包，目录名即 artifact 名
        let final_dir = output_dir.join(&base_name);
        let _ = fs::remove_dir_all(&final_dir);
        fs::rename(&temp_dir, &final_dir)?;
        println!("模块目录已生成: {}", final_dir.display());
    } else {
        let zip_path = output_dir.join(format!("{base_name}.zip"));
        println!("开始打包: {}", zip_path.display());

        let method = zip_method()?;
        let options: FileOptions<'_, ()> = FileOptions::default()
            .compression_method(method)
            .compression_level(Some(zip_level(method)));

        zip_create_from_directory_with_options(&zip_path, &temp_dir, |_| options)?;
    }

    println!("构建成功");
    Ok(())
}

// [pack-flow]

/// 把已组装好的模块目录打成 zip。
///
/// 命名沿用本地构建脚本的 `<目录名>-b<分秒>` 约定，压缩算法与 `build` 共用
/// [`zip_method`]（默认 XZ）。末行输出 `zip=<路径>` 供脚本抓取。
fn pack(dir: Option<PathBuf>) -> Result<()> {
    let source = match dir {
        Some(p) => p,
        None => latest_output_dir()?,
    };
    if !source.is_dir() {
        anyhow::bail!("待打包目录不存在: {}", source.display());
    }
    let name = source
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("目录名不是合法 UTF-8: {}", source.display()))?;
    let stamp = chrono::Local::now().format("%M%S");
    let zip_path = Path::new("output").join(format!("{name}-b{stamp}.zip"));

    let method = zip_method()?;
    let options: FileOptions<'_, ()> = FileOptions::default()
        .compression_method(method)
        .compression_level(Some(zip_level(method)));
    zip_create_from_directory_with_options(&zip_path, &source, |_| options)?;

    let bytes = fs::metadata(&zip_path)?.len();
    println!(
        "打包完成: {} ({:.2} MiB, {}, level {})",
        zip_path.display(),
        bytes as f64 / (1024.0 * 1024.0),
        method_name(method),
        zip_level(method)
    );
    println!("zip={}", zip_path.display());
    Ok(())
}

/// output/ 下最近修改的模块目录（`build --no-pack` 的产物）
fn latest_output_dir() -> Result<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir("output")? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // 跳过工作用目录（build 的 .temp、校验解包目录等一律以 . 开头）
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }
        let modified = entry.metadata()?.modified()?;
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, path));
        }
    }
    match best {
        Some((_, path)) => Ok(path),
        None => anyhow::bail!("output/ 下没有模块目录，先跑 cargo xtask build --no-pack"),
    }
}

// [sub-builders]

/// 打包压缩算法：本地默认 XZ（zip method 95，级别见 `zip_level`）。
///
/// 体积比 Deflate（method 8、level 9）小约 20%（实测同目录 3,530,873 → 2,841,480 B，−19.5%）；三家管理器都能解：
/// Magisk 的 busybox 1.36.1 编了 `FEATURE_UNZIP_XZ`、KernelSU 的 ksud 用 zip 8 且开 `xz`、
/// APatch 的 apd 用 zip 8.6 且开 `xz`。CI 只组装目录（`--no-pack`），不走本分支，故 CI 不受影响；
/// 需要 Deflate 兼容包时设 `CHIRI_ZIP_METHOD=deflate`。
fn zip_method() -> Result<CompressionMethod> {
    match env::var("CHIRI_ZIP_METHOD").as_deref() {
        Ok("deflate") => Ok(CompressionMethod::Deflated),
        Ok("xz") | Err(_) => Ok(CompressionMethod::Xz),
        Ok(other) => anyhow::bail!("未知的 CHIRI_ZIP_METHOD `{other}`：只接受 xz / deflate"),
    }
}

/// 压缩级别：Deflate 用 9（最大），XZ 用 6。
///
/// XZ 的 preset 决定 LZMA2 字典：preset 6 = 8 MiB、preset 9 = 64 MiB。本包最大成员是
/// `core/bin/chiri`（约 6 MiB），装得进 8 MiB 字典就用不满更大的，实测 preset 6/7/8/9
/// 输出**逐字节相同**；而设备端解压时字典越大占用越多内存，故取 6。
fn zip_level(m: CompressionMethod) -> i64 {
    match m {
        CompressionMethod::Xz => 6,
        _ => 9,
    }
}

/// 压缩算法在日志里的短名
fn method_name(m: CompressionMethod) -> &'static str {
    match m {
        CompressionMethod::Xz => "xz",
        CompressionMethod::Deflated => "deflate",
        _ => "other",
    }
}

fn temp_dir() -> PathBuf {
    Path::new("output").join(".temp")
}

fn aarch64_bin_path() -> PathBuf {
    Path::new("target")
        .join("aarch64-linux-android")
        .join("release")
        .join("chiri")
}

/// 核心模块（chiri）：独立 cargo 构建单元。
/// Fixed 模式失败直接上报；Auto 模式按模块回退 thin → off，全部失败时报告第一档的错误。
fn build_core(sh: &Shell, plan: LtoPlan) -> Result<()> {
    match plan {
        LtoPlan::Fixed(level) => run_core(sh, level, false),
        LtoPlan::Auto => {
            let mut first_error = None;
            let mut level = LtoLevel::Full;
            loop {
                match run_core(sh, level, true) {
                    Ok(()) => {
                        if level != LtoLevel::Full {
                            println!(
                                "⚠ chiri 已回退到 LTO={}（full 档构建失败，见上方日志）",
                                level.as_str()
                            );
                        }
                        return Ok(());
                    }
                    Err(err) => {
                        first_error.get_or_insert(err);
                        let Some(next) = level.next() else { break };
                        println!(
                            "⚠ chiri 在 LTO={} 下构建失败，按模块回退 {} 重试",
                            level.as_str(),
                            next.as_str()
                        );
                        level = next;
                    }
                }
            }
            let err = first_error.expect("full 档必然已尝试");
            Err(err.context("chiri 在 full / thin / off 三档 LTO 下均构建失败"))
        }
    }
}

/// 跑一次核心构建。`fallback_allowed` 只影响下发给 build.rs 的 eBPF 模块策略：
/// Auto 时 eBPF 也允许独立回退；本地固定档时 eBPF 必须使用同一档。
fn run_core(sh: &Shell, level: LtoLevel, fallback_allowed: bool) -> Result<()> {
    println!("正在编译 Rust Core（chiri，LTO={}）", level.as_str());
    // Cargo 用 CARGO_PROFILE_RELEASE_LTO 覆盖 [profile.release].lto（full 必须写成 fat）
    let _lto = sh.push_env("CARGO_PROFILE_RELEASE_LTO", level.cargo_value());
    // 下发给 build.rs（eBPF 模块）：等级 + 是否允许该模块自行回退
    let _module_lto = sh.push_env(LTO_ENV, level.as_str());
    let _module_fallback = sh.push_env(
        "CHIRI_LTO_FALLBACK",
        if fallback_allowed { "1" } else { "0" },
    );
    // push_env 在当前作用域内设置环境变量，离开作用域自动恢复
    let _env = sh.push_env("RUSTFLAGS", "-C default-linker-libraries");
    // 不写 `+<spec>`：`+` 的优先级高于仓库根的 rust-toolchain.toml，
    // 一旦这里钉的 spec 与文件不同（例如 `+nightly` 浮动 vs 文件里钉的日期版），
    // 就会解析到两个不同的编译器，共用 target/ 时整片报 E0514。
    // 工具链统一由 rust-toolchain.toml 决定，裸 `cargo` 即可（2026-10-22 收口）。
    cmd!(
        sh,
        "cargo ndk --platform 26 -t arm64-v8a build -Z build-std -r"
    )
    .run()?;
    Ok(())
}

fn build_webui(sh: &Shell) -> Result<()> {
    println!("正在编译 WebUI");
    // push_dir 类似 cd，离开作用域后自动切回原目录
    let _dir = sh.push_dir("webui");
    cmd!(sh, "npm run build").run()?;
    Ok(())
}

// [strip] 在产物副本上删整行注释

/// 删模块文本文件中的整行 `#` 注释，保留 shebang（`#!`）、行内 `#`
/// 只应在 `temp_dir` 副本上调用：`module/` 是唯一真身，构建中断不得污染它
/// 排除 `META-INF/`（updater-script 的 `#MAGISK` 为必需标记）与 `scripts/`（对外稳定接口，注释保留便于排障），二者只在顶层生效
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

    /// 用户口径 full 必须映射成 cargo 的 fat（rustc 拒绝 `lto=full`）
    #[test]
    fn lto_full_maps_to_cargo_fat() {
        assert_eq!(LtoLevel::Full.cargo_value(), "fat");
        assert_eq!(LtoLevel::Thin.cargo_value(), "thin");
        assert_eq!(LtoLevel::Off.cargo_value(), "off");
    }

    /// CI 的按模块回退阶梯：full → thin → off → 结束
    #[test]
    fn lto_fallback_ladder_is_full_then_thin_then_off() {
        assert_eq!(LtoLevel::Full.next(), Some(LtoLevel::Thin));
        assert_eq!(LtoLevel::Thin.next(), Some(LtoLevel::Off));
        assert_eq!(LtoLevel::Off.next(), None);
    }

    /// CLI/env 接受用户口径与 cargo 别名（大小写、空白不敏感）；auto 由上层解析
    #[test]
    fn lto_parse_accepts_user_and_cargo_aliases() {
        for raw in ["full", "FAT", " true "] {
            assert_eq!(LtoLevel::parse(raw), Some(LtoLevel::Full), "{raw}");
        }
        assert_eq!(LtoLevel::parse("thin"), Some(LtoLevel::Thin));
        for raw in ["off", "false", "none"] {
            assert_eq!(LtoLevel::parse(raw), Some(LtoLevel::Off), "{raw}");
        }
        assert_eq!(LtoLevel::parse("auto"), None);
        assert_eq!(LtoLevel::parse(""), None);
    }

    /// 与历史 CI 谓词等价的语义：整行 # 删；shebang、行内 #、META-INF/、scripts/ 保留
    #[test]
    fn strip_comments_semantics() {
        let root = Path::new("target").join("xtask-strip-test");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("META-INF")).unwrap();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::create_dir_all(root.join("foo")).unwrap();

        fs::write(
            root.join("a.sh"),
            "#!/bin/sh\n# comment\n  # indented\nx=1 # inline\n",
        )
        .unwrap();
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
        assert_eq!(
            fs::read_to_string(root.join("META-INF/keep.sh")).unwrap(),
            "#META\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("scripts/keep.sh")).unwrap(),
            "# c\nrun\n"
        );

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
