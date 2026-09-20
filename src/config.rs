//! 配置文件读写。
//!
//! 配置放在 `~/.config/pory/config.toml`（Linux/macOS）
//! 或 `%APPDATA%\pory\config.toml`（Windows）—— 由 `directories` crate
//! 按各平台惯例决定，不硬编码路径。
//!
//! 设计原则：**配置缺失不是错误**。找不到配置文件就用内置默认值，
//! 这样 `` 开箱即用，不需要先跑什么 init 命令。

use crate::error::{PoryError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 配置文件内容
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 默认使用哪个后端：mymemory / google / ai
    pub backend: String,

    /// 主后端失败时依次尝试的备选后端。
    ///
    /// 这就是「保底」机制的落点：比如主后端配成 `ai`（质量最好但要 Key、
    /// 可能超额或断网），把 `mymemory` 放在这里，AI 挂掉时自动兜住，
    /// 而不是直接报错让你没得用。
    ///
    /// 空数组表示不回退 —— 主后端失败就失败。
    pub fallback: Vec<String>,

    /// 第一语言（你的母语）：不指定 `-t` 时的默认翻译目标。
    ///
    /// 字段名曾叫 `target`，用 serde 的 `alias` 兼容老配置 —— 老文件不必改。
    #[serde(alias = "target")]
    pub primary: String,

    /// 第二语言：与第一语言互翻。
    ///
    /// auto 模式（既没给 `-f` 也没给 `-t`）下，若检测出原文已经是第一语言，
    /// 就改译成它 —— 省掉「中文翻中文」这种没有意义的请求。
    /// 留空字符串表示关闭互翻（此时原文已是第一语言就原样返回）。
    pub secondary: String,

    /// 默认源语言，auto 表示自动检测
    pub source: String,

    /// 是否使用翻译缓存。
    ///
    /// 默认 `true`。设为 `false` 等价于每次调用都带上 `--no-cache` ——
    /// 既不读已有译文，也不写入新的，每次都是真实请求。
    ///
    /// 与命令行那三个开关的分工：命令行管「这一次」（`--no-cache` 本次不用、
    /// `--refresh` 本次重来、`--clear-cache` 清空磁盘存量），这里管「以后每次都这样」。
    pub cache: bool,

    /// MyMemory 专用：联系邮箱（填了能提额度）
    pub mymemory_email: Option<String>,

    /// Google 专用：自定义接口地址（国内反代用）
    pub google_endpoint: Option<String>,

    /// AI 后端配置
    pub ai: AiConfig,
}

/// AI 后端的配置块
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    /// 接口根地址（OpenAI 兼容）
    pub base_url: String,
    /// API Key
    pub api_key: String,
    /// 模型名
    pub model: String,
}

impl Default for Config {
    /// 内置默认值。目标是「什么都没配也能跑」。
    fn default() -> Self {
        Self {
            // MyMemory 免 Key 直连，是最可靠的保底
            backend: "mymemory".to_string(),
            // 默认回退到 MyMemory —— 它免 Key、国内直连，是最可靠的兜底。
            // 若主后端本身就是 mymemory，构建后端链时会自动去重。
            fallback: vec!["mymemory".to_string()],
            // 默认「母语中文、另一门英文」（面向中文用户）。
            // 这与 target 默认 zh 是同一种取向：不做中立假设，直接给可用默认值。
            primary: "zh".to_string(),
            secondary: "en".to_string(),
            source: "auto".to_string(),
            // 默认开缓存 —— 不写这一项时行为与加它之前完全一致（opt-in 约定）
            cache: true,
            mymemory_email: None,
            google_endpoint: None,
            ai: AiConfig::default(),
        }
    }
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.deepseek.com/v1".to_string(),
            api_key: String::new(),
            model: "deepseek-chat".to_string(),
        }
    }
}

impl Config {
    /// 定位配置文件路径（不一定存在）
    pub fn path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "pory")
            .ok_or_else(|| PoryError::Config("无法定位用户配置目录".into()))?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// 加载配置。文件不存在或解析失败都回落到默认值。
    ///
    /// 注意这里「解析失败」也只是警告 + 用默认值，而不是直接报错退出。
    /// 理由：一个工具不该因为配置文件里一个笔误就完全不能用。
    pub fn load() -> Self {
        let Ok(path) = Self::path() else {
            return Self::default();
        };

        if !path.exists() {
            return Self::default();
        }

        match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str::<Config>(&content) {
                Ok(cfg) => cfg,
                Err(e) => {
                    eprintln!("⚠ 配置文件解析失败，改用默认配置：{e}");
                    Self::default()
                }
            },
            Err(e) => {
                eprintln!("⚠ 配置文件读取失败，改用默认配置：{e}");
                Self::default()
            }
        }
    }

    /// 生成一份带注释的示例配置，供 `pory --init` 使用
    pub fn template() -> String {
        r#"# pory 配置文件
# 位置：~/.config/pory/config.toml（Linux/macOS）
#       %APPDATA%\pory\config.toml（Windows）

# 使用哪个翻译后端：mymemory | google | ai
#   mymemory —— 免 Key、国内直连，默认选择
#   google   —— 免 Key、质量好，但国内需要代理
#   ai       —— 质量最好、懂上下文，需要 API Key
backend = "mymemory"

# 保底机制：主后端失败时，按顺序依次尝试这里列出的后端。
# 典型用法：backend = "ai"，fallback = ["mymemory"]
#   → AI 因没 Key / 超额 / 断网失败时，自动用 MyMemory 兜住
# 空数组表示没有兜底：主后端不可用时就只能报错。
fallback = ["mymemory"]

# 第一语言（你的母语）：不指定 -t 时翻译成它
primary = "zh"

# 第二语言：与第一语言互翻。
# auto 模式（既不写 -f、也不写 -t）下，若检测出原文已经是第一语言，
# 就自动改译成第二语言 —— 避免「中文翻中文」这种没有意义的请求。
# 留空 "" 表示关闭互翻：原文已是第一语言时原样返回，不做二次请求。
secondary = "en"

# 默认源语言，auto 表示自动检测
source = "auto"

# 是否使用翻译缓存（默认 true）。
# 设为 false 等价于每次调用都带上 --no-cache：既不读已有译文，也不写入新的。
# 适合两种场景：不想在磁盘上留缓存文件；或做对照测试、需要每次真实请求。
cache = true


# ── MyMemory 设置 ──
# 填一个邮箱能提高每日免费额度（官方推荐）
# mymemory_email = "you@example.com"


# ── Google 设置 ──
# 国内可填自己搭的反代地址（Cloudflare Worker 等）
# google_endpoint = "https://your-worker.workers.dev/translate_a/single"


# ── AI 后端设置（OpenAI 兼容协议）──
# 换 base_url 即可切换服务商：
#   DeepSeek  https://api.deepseek.com/v1
#   Kimi      https://api.moonshot.cn/v1
#   智谱      https://open.bigmodel.cn/api/paas/v4
#   本地Ollama http://localhost:11434/v1
[ai]
base_url = "https://api.deepseek.com/v1"
api_key = ""
model = "deepseek-chat"
"#
        .to_string()
    }

    /// 把示例配置写到磁盘（用于 `--init`）。
    ///
    /// 是关联函数而非方法：生成模板不需要已有配置实例。
    pub fn save() -> Result<PathBuf> {
        let path = Self::path()?;

        // 确保父目录存在
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| PoryError::Config(format!("无法创建配置目录 {}：{e}", parent.display())))?;
        }

        // 已经有配置就不覆盖，避免冲掉用户自己的设置
        if path.exists() {
            return Err(PoryError::Config(format!(
                "配置文件已存在：{}（未覆盖）",
                path.display()
            )));
        }

        std::fs::write(&path, Self::template())
            .map_err(|e| PoryError::Config(format!("写入配置失败：{e}")))?;

        Ok(path)
    }

    /// 把默认后端写进配置文件（`pory -b <名字>` 不带文本时用）。
    ///
    /// 用**文本级行替换**，不用「反序列化 → 改字段 → 序列化写回」：
    /// 后者会把配置里的注释和排版全部抹掉，而 `--init` 生成的模板满是中文注释
    /// （fallback 的用法、AI 各家的 base_url 都写在注释里），
    /// 抹掉等于毁掉这份配置的说明书。所以只改顶层 `backend = ...` 那一行，其余原样保留。
    pub fn set_backend(name: &str) -> Result<PathBuf> {
        let path = Self::path()?;

        // 还没有配置文件 → 先落一份完整模板（与 `--init` 完全一致），
        // 这样用户以后想填 API Key、调 fallback 时都有现成的注释可照。
        if !path.exists() {
            Self::save()?;
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| PoryError::Config(format!("读取配置失败 {}：{e}", path.display())))?;

        let updated = replace_top_level(&content, "backend", name);

        std::fs::write(&path, updated)
            .map_err(|e| PoryError::Config(format!("写入配置失败 {}：{e}", path.display())))?;

        Ok(path)
    }
}

/// 把 TOML 里**顶层**的 `key = "..."` 换成新值，其余内容（注释、空行、排版）原样保留。
///
/// 为什么必须判「顶层」：TOML 里段一旦开始，后面同名的 key 就是那个表的字段
/// （比如 `[ai]` 段里再来一个 `backend`），改到它就不是我们想要的了。
/// 所以只在**第一个 `[section]` 之前**找；找不到就插在第一个段之前。
/// 绝不能盲目 append 到文件末尾 —— 那会落进最后一个段里，变成 `ai.backend`。
fn replace_top_level(content: &str, key: &str, value: &str) -> String {
    let new_line = format!("{key} = \"{value}\"");
    let mut out = String::with_capacity(content.len() + new_line.len() + 1);
    let mut replaced = false;
    let mut in_section = false;

    for line in content.lines() {
        let trimmed = line.trim_start();

        if trimmed.starts_with('[') {
            // 遇到第一个段标记：还没写进去的话，就插在它之前
            if !replaced {
                out.push_str(&new_line);
                out.push('\n');
                replaced = true;
            }
            in_section = true;
        }

        if !in_section && !replaced && is_key_line(trimmed, key) {
            out.push_str(&new_line);
            out.push('\n');
            replaced = true;
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }

    // 整个文件都是顶层字段、且没有这一项 → 追加到末尾
    if !replaced {
        out.push_str(&new_line);
        out.push('\n');
    }

    out
}

/// 判断一行是不是 `key = ...` 形式的字段赋值。
///
/// 要排除两种貌似同名的情况：注释掉的 `# backend = ...`，
/// 以及名字里含该 key 的别的字段（`my_backend = ...`）。
fn is_key_line(trimmed: &str, key: &str) -> bool {
    if trimmed.starts_with('#') {
        return false;
    }
    match trimmed.split_once('=') {
        Some((k, _)) => k.trim() == key,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 替换顶层字段且保留注释() {
        let src = "# 说明\nbackend = \"mymemory\"\n# 另一行\nsource = \"auto\"\n";
        let out = replace_top_level(src, "backend", "google");
        assert!(out.contains("backend = \"google\""));
        assert!(out.contains("# 说明"));
        assert!(out.contains("# 另一行"));
        assert!(out.contains("source = \"auto\""));
        assert!(!out.contains("mymemory"));
    }

    #[test]
    fn 不动段内的同名字段() {
        // [ai] 段里的 backend 是那个表的字段，不该被改；
        // 顶层没有 backend → 新行要插在段标记之前，不能落进段里
        let src = "source = \"auto\"\n\n[ai]\nbackend = \"别动我\"\n";
        let out = replace_top_level(src, "backend", "google");

        assert!(out.contains("backend = \"别动我\""));
        assert!(out.contains("backend = \"google\""));
        assert!(
            out.find("backend = \"google\"").unwrap() < out.find("[ai]").unwrap(),
            "新行必须插在段标记之前"
        );
    }

    #[test]
    fn 注释掉的字段不算数() {
        let src = "# backend = \"注释里的\"\nsource = \"auto\"\n";
        let out = replace_top_level(src, "backend", "google");
        assert!(out.contains("# backend = \"注释里的\""));
        assert!(out.trim_end().ends_with("backend = \"google\""));
    }

    #[test]
    fn 名字相近的字段不受影响() {
        let src = "my_backend = \"x\"\nbackend = \"mymemory\"\n";
        let out = replace_top_level(src, "backend", "google");
        assert!(out.contains("my_backend = \"x\""));
        assert!(out.contains("backend = \"google\""));
        assert!(!out.contains("mymemory"));
    }

    #[test]
    fn 没有段时追加到末尾() {
        let out = replace_top_level("source = \"auto\"\n", "backend", "google");
        assert!(out.contains("source = \"auto\""));
        assert!(out.trim_end().ends_with("backend = \"google\""));
    }

    /// 拿真实模板跑一遍：既要改对值，又必须仍是合法 TOML、注释一条不少。
    /// 这条是「写配置不能毁掉配置」的守门测试。
    #[test]
    fn 在真实模板上替换后仍是合法配置() {
        let out = replace_top_level(&Config::template(), "backend", "google");
        let cfg: Config = toml::from_str(&out).expect("替换后应仍能解析");
        assert_eq!(cfg.backend, "google");
        // 模板里的注释是这份配置唯一的说明书，不能丢
        assert!(out.contains("保底机制"));
        assert!(out.contains("第一语言"));
    }
}
