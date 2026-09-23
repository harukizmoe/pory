//! 配置文件读写。
//!
//! 配置放在 `~/.config/pory/config.toml`（Linux/macOS）
//! 或 `%APPDATA%\pory\config.toml`（Windows）—— 由 `directories` crate
//! 按各平台惯例决定，不硬编码路径。
//!
//! 设计原则：**配置缺失不是错误**。找不到配置文件就用内置默认值，
//! 这样开箱即用，不需要先跑什么 init 命令。
//!
//! ## 双模式路由（2026-09-21 定稿）
//!
//! 翻译模式只有两类：`ai`（OpenAI 兼容大模型，可配多个提供商）与
//! `traditional`（免 Key 机翻：微软 / 腾讯 / MyMemory / Google）。
//! 默认 `ai`，AI 全部不可用时自动落回 traditional 兜底 ——
//! 路由只是**构建后端链的策略**（见 main.rs 的 build_mode_chain），
//! 调度与回退机制本身不变。
//!
//! 2026-09-21 起不再兼容旧的 `backend` / `fallback` 字段（用户拍板）。

use crate::error::{PoryError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 翻译模式。
///
/// 序列化成小写字符串（`"ai"` / `"traditional"`），与配置文件里的写法一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// AI 翻译：按 `[ai]` 的 order 依次尝试，全挂落传统
    Ai,
    /// 传统机翻：只用 `[traditional]` 的链，完全不碰 AI
    Traditional,
}

/// 配置文件内容
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 翻译模式：ai（默认，AI 优先、传统兜底）| traditional（只用传统机翻）
    pub mode: Mode,

    /// 第一语言（你的母语）：不指定 `-t` 时的默认翻译目标。
    ///
    /// 字段名曾叫 `target`，用 serde 的 `alias` 兼容更老的配置。
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

    /// AI 模式：提供商列表与各自的接入配置
    pub ai: AiSection,

    /// 传统模式：免 Key 机翻的链顺序
    pub traditional: TraditionalSection,
}

/// AI 模式的配置块。
///
/// TOML 形态（注意 `[ai.zhipu]` 直接就是提供商，不需要再多一层）：
///
/// ```toml
/// [ai]
/// order = ["zhipu"]
///
/// [ai.zhipu]
/// base_url = "https://open.bigmodel.cn/api/paas/v4"
/// model = "glm-4.7-flash"
/// api_key = "..."
/// ```
///
/// `order` 是显式字段；其余所有子表靠 serde `flatten` 收集进 `providers`。
/// 只按 order 里的名字 `get()` 查找、从不遍历 providers，所以用 HashMap 即可
/// （遍历顺序不定对我们无影响）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSection {
    /// 尝试顺序。没填 api_key 的提供商自动跳过。默认空 —— 开箱靠传统兜底。
    pub order: Vec<String>,

    /// 名字 → 接入配置。名字限 `a-z 0-9 -`（会进缓存键与脚注）。
    #[serde(flatten)]
    pub providers: HashMap<String, ProviderConfig>,
}

/// 单个 AI 提供商的接入配置（OpenAI 兼容协议）
///
/// **一个提供商（URL + Key）可以对应多个模型**：`models` 是列表，
/// 构建链时按声明顺序展开为多个「提供商:模型」实例 ——
/// 同提供商的主力模型挂了，先试它的备选模型（省一次 TLS 建连与换 Key），再轮到下一家提供商。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// 接口根地址，比如 https://open.bigmodel.cn/api/paas/v4
    pub base_url: String,
    /// API Key。留空 = 该提供商不参与路由（构建时跳过并警告）
    pub api_key: String,
    /// 可用模型列表。所有模型都进链、都可用（顺序即尝试顺序）
    pub models: Vec<String>,
    /// 思考模式开关（三态）。
    ///
    /// - `None`（缺省）：**不传参数**，用服务商的默认行为；
    /// - `Some(false)`：请求体带 `enable_thinking = false`；
    /// - `Some(true)`：请求体带 `enable_thinking = true`。
    ///
    /// 实测依据（2026-09-21，硅基流动）：Qwen3-8B 关思考后 22.9s → 1.43s
    /// （输出 658 → 14 tokens）—— 思考是 AI 翻译慢的头号原因；
    /// 翻译专用模型（Hunyuan-MT）不认识该参数但会**安全忽略**，传了无害。
    ///
    /// 该参数按硅基流动的 `enable_thinking` 字段实现；不支持此字段的服务商
    /// 通常忽略未知参数（OpenAI 兼容层的惯例），如遇报错把它删掉即可。
    /// 影响输出 → 必须进缓存键（见 ai.rs 的 cache_detail）。
    pub thinking: Option<bool>,
}

/// 传统模式的配置块：免 Key 机翻的链顺序。
///
/// 合法的后端名（由 `backend::build_traditional` 认）：
/// `msedge` / `transmart` / `mymemory` / `google`。
/// 写了未知名只警告跳过，不让整个工具罢工 —— 与「配置缺失不是错误」同一条哲学。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TraditionalSection {
    pub order: Vec<String>,
}

/// 传统后端的合法名单，用于错误提示。
pub const TRADITIONAL_KNOWN: [&str; 4] = ["msedge", "transmart", "mymemory", "google"];

/// provider 名的字符白名单。
///
/// 这个名字会进缓存键和脚注，只放行保守的字符集；
/// TOML 表名本身合法性由 toml 解析器把关，这里只做额外收紧。
pub fn is_valid_provider_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl Default for Config {
    /// 内置默认值。目标是「什么都没配也能跑」。
    ///
    /// mode 默认 `Ai`：没有配置任何 AI 提供商时，链构建会自动落到
    /// traditional 并给出一条警告 —— 工具照常可用（开箱即用不依赖 Key）。
    fn default() -> Self {
        Self {
            mode: Mode::Ai,
            // 默认「母语中文、另一门英文」（面向中文用户）。
            // 这与 target 默认 zh 是同一种取向：不做中立假设，直接给可用默认值。
            primary: "zh".to_string(),
            secondary: "en".to_string(),
            source: "auto".to_string(),
            // 默认开缓存 —— 不写这一项时行为与加它之前完全一致（opt-in 约定）
            cache: true,
            mymemory_email: None,
            google_endpoint: None,
            ai: AiSection::default(),
            traditional: TraditionalSection::default(),
        }
    }
}

// AiSection 的 Default 已由 derive 生成：order 空、providers 空 ——
// 开箱没有任何提供商，靠传统兜底，想用 AI 自己加。

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            // 默认给智谱：免费档（glm-4.7-flash 永久免费），注册即得 Key
            base_url: "https://open.bigmodel.cn/api/paas/v4".to_string(),
            api_key: String::new(),
            models: vec!["glm-4.7-flash".to_string()],
            // 默认不传思考参数：服务商怎么默认就怎么来（与引入该开关前行为一致）
            thinking: None,
        }
    }
}

impl Default for TraditionalSection {
    fn default() -> Self {
        Self {
            // 默认顺序的取舍：微软质量最好且国内直连；腾讯最快；
            // MyMemory 是老保底（1000 次/天）；Google 要代理、有风控，排末位。
            order: TRADITIONAL_KNOWN.iter().map(|s| s.to_string()).collect(),
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

# 翻译模式：ai | traditional
#   ai          —— 大模型翻译，懂上下文、质量最好；提供商需填 api_key
#   traditional —— 免 Key 机翻（微软 / 腾讯 / MyMemory / Google），永远可用
# 默认 ai：AI 提供商全部未配置或失败时，自动落回 traditional 兜底，
# 不会让工具罢工 —— 兜底发生时脚注里看得见（变琥珀色）。
mode = "ai"

# ── AI 提供商（OpenAI 兼容协议，可配多个）──
# order 决定提供商的尝试顺序；没填 api_key 的自动跳过。
# 一个提供商（URL + Key）可配多个模型（models 列表）：
# 同提供商的主力模型挂了，先试它的备选模型，再轮到下一家提供商。
# 提供商名字限小写字母 / 数字 / 连字符（会进缓存键和脚注）。
#
# thinking（可选）：AI 思考模式开关。
#   不写               = 不传参数，用服务商默认（Qwen3 系默认开思考，翻译慢很多）
#   thinking = false   = 请求带 enable_thinking=false，关掉思考
# 实测（硅基流动 2026-09-21）：Qwen3-8B 关思考 22.9s → 1.4s；
# 翻译专用模型（Hunyuan-MT）不认识该参数但会安全忽略，传了无害。
#
# 免费档速查（2026-09 核实，政策易变，以各家控制台为准）：
#   智谱 GLM    https://open.bigmodel.cn/api/paas/v4    glm-4.7-flash（永久免费，限 1 并发）
#   硅基流动    https://api.siliconflow.cn/v1           THUDM/glm-4-9b-chat（9B 以下永久免费）
#   DeepSeek   https://api.deepseek.com/v1             deepseek-chat（付费）
#   本地 Ollama http://localhost:11434/v1               模型名随意（免 Key）
[ai]
order = []

# 示例：智谱（注册 open.bigmodel.cn 即得免费 Key）
# [ai.zhipu]
# base_url = "https://open.bigmodel.cn/api/paas/v4"
# api_key = ""
# models = ["glm-4.7-flash", "glm-4.5-flash"]

# 示例：硅基流动（注册 cloud.siliconflow.cn，9B 以下模型免费）
# [ai.siliconflow]
# base_url = "https://api.siliconflow.cn/v1"
# api_key = ""
# models = ["tencent/Hunyuan-MT-7B", "Qwen/Qwen3-8B"]
# thinking = false

# ── 传统机翻（免 Key，永远可用）──
# order 同时决定 mode=traditional 时的链顺序，和 mode=ai 时的兜底顺序。
#   msedge    微软 Edge 翻译端点：质量最好、国内直连（非官方接口，无稳定性承诺）
#   transmart 腾讯交互翻译：快、国内直连（非官方接口，无稳定性承诺）
#   mymemory  老牌免费 MT：1000 次/天（填 mymemory_email 可提额）
#   google    质量好但国内需代理，有 429 风控
[traditional]
order = ["msedge", "transmart", "mymemory", "google"]

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

# ── 零散设置 ──
# MyMemory：填一个邮箱能提高每日免费额度（官方推荐）
# mymemory_email = "you@example.com"
# Google：国内可填自己搭的反代地址（Cloudflare Worker 等）
# google_endpoint = "https://your-worker.workers.dev/translate_a/single"
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
            std::fs::create_dir_all(parent).map_err(|e| {
                PoryError::Config(format!("无法创建配置目录 {}：{e}", parent.display()))
            })?;
        }

        // 已经有配置就不覆盖，避免冲掉用户自己的设置
        if path.exists() {
            return Err(PoryError::Config(format!(
                "配置文件已存在：{}（未覆盖）",
                path.display()
            )));
        }

        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // 配置中可能包含 API Key。不要让常见的 umask 022 创建出 0644 文件。
            options.mode(0o600);
        }

        let mut file = options.open(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                PoryError::Config(format!("配置文件已存在：{}（未覆盖）", path.display()))
            } else {
                PoryError::Config(format!("写入配置失败 {}：{e}", path.display()))
            }
        })?;
        file.write_all(Self::template().as_bytes())
            .map_err(|e| PoryError::Config(format!("写入配置失败 {}：{e}", path.display())))?;

        Ok(path)
    }

    /// 把默认翻译模式写进配置文件（`pory -b <模式>` 不带文本时用）。
    ///
    /// 用**文本级行替换**，不用「反序列化 → 改字段 → 序列化写回」：
    /// 后者会把配置里的注释和排版全部抹掉，而 `--init` 生成的模板满是中文注释
    /// （免费提供商速查、传统后端的取舍说明都写在注释里），
    /// 抹掉等于毁掉这份配置的说明书。所以只改顶层 `mode = ...` 那一行，其余原样保留。
    pub fn set_mode(mode: Mode) -> Result<PathBuf> {
        let path = Self::path()?;

        // 还没有配置文件 → 先落一份完整模板（与 `--init` 完全一致），
        // 这样用户以后想填 API Key、调 order 时都有现成的注释可照。
        if !path.exists() {
            Self::save()?;
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| PoryError::Config(format!("读取配置失败 {}：{e}", path.display())))?;

        let updated = replace_top_level(&content, "mode", mode_marker(mode));
        restrict_config_permissions(&path)?;

        std::fs::write(&path, updated)
            .map_err(|e| PoryError::Config(format!("写入配置失败 {}：{e}", path.display())))?;

        Ok(path)
    }
}

/// 收紧已有配置文件的权限。设置默认模式时也会修正老配置文件的权限。
#[cfg(unix)]
fn restrict_config_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)
        .map_err(|e| PoryError::Config(format!("读取配置文件权限失败 {}：{e}", path.display())))?
        .permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions)
        .map_err(|e| PoryError::Config(format!("收紧配置文件权限失败 {}：{e}", path.display())))
}

#[cfg(not(unix))]
fn restrict_config_permissions(_path: &Path) -> Result<()> {
    // Windows 使用继承的用户 ACL；Unix 的 0600 权限位不适用。
    Ok(())
}

/// Mode 的配置文件写法（小写字符串，与 serde 序列化一致）
fn mode_marker(mode: Mode) -> &'static str {
    match mode {
        Mode::Ai => "ai",
        Mode::Traditional => "traditional",
    }
}

impl std::fmt::Display for Mode {
    /// 显示为配置文件里的写法（ai / traditional）——
    /// 提示文案里的模式名必须和用户在配置里看到的一致，别另造一套叫法。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(mode_marker(*self))
    }
}

/// 把 TOML 里**顶层**的 `key = "..."` 换成新值，其余内容（注释、空行、排版）原样保留。
///
/// 为什么必须判「顶层」：TOML 里段一旦开始，后面同名的 key 就是那个表的字段
/// （比如 `[ai]` 段里再来一个 `mode`），改到它就不是我们想要的了。
/// 所以只在**第一个 `[section]` 之前**找；找不到就插在第一个段之前。
/// 绝不能盲目 append 到文件末尾 —— 那会落进最后一个段里，变成 `ai.mode`。
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
/// 要排除两种貌似同名的情况：注释掉的 `# mode = ...`，
/// 以及名字里含该 key 的别的字段（`my_mode = ...`）。
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
        let src = "# 说明\nmode = \"ai\"\n# 另一行\nsource = \"auto\"\n";
        let out = replace_top_level(src, "mode", "traditional");
        assert!(out.contains("mode = \"traditional\""));
        assert!(out.contains("# 说明"));
        assert!(out.contains("# 另一行"));
        assert!(out.contains("source = \"auto\""));
        assert!(!out.contains("mode = \"ai\"\n# 另一行"));
    }

    #[test]
    fn 不动段内的同名字段() {
        // [ai] 段里的 mode 是那个表的字段，不该被改；
        // 顶层没有 mode → 新行要插在段标记之前，不能落进段里
        let src = "source = \"auto\"\n\n[ai]\nmode = \"别动我\"\n";
        let out = replace_top_level(src, "mode", "traditional");

        assert!(out.contains("mode = \"别动我\""));
        assert!(out.contains("mode = \"traditional\""));
        assert!(
            out.find("mode = \"traditional\"").unwrap() < out.find("[ai]").unwrap(),
            "新行必须插在段标记之前"
        );
    }

    #[test]
    fn 注释掉的字段不算数() {
        let src = "# mode = \"注释里的\"\nsource = \"auto\"\n";
        let out = replace_top_level(src, "mode", "traditional");
        assert!(out.contains("# mode = \"注释里的\""));
        assert!(out.trim_end().ends_with("mode = \"traditional\""));
    }

    #[test]
    fn 名字相近的字段不受影响() {
        let src = "my_mode = \"x\"\nmode = \"ai\"\n";
        let out = replace_top_level(src, "mode", "traditional");
        assert!(out.contains("my_mode = \"x\""));
        assert!(!out.contains("mode = \"ai\""));
    }

    #[test]
    fn 没有段时追加到末尾() {
        let out = replace_top_level("source = \"auto\"\n", "mode", "traditional");
        assert!(out.contains("source = \"auto\""));
        assert!(out.trim_end().ends_with("mode = \"traditional\""));
    }

    /// 拿真实模板跑一遍：既要改对值，又必须仍是合法 TOML、注释一条不少。
    /// 这条是「写配置不能毁掉配置」的守门测试。
    #[test]
    fn 在真实模板上替换后仍是合法配置() {
        let out = replace_top_level(&Config::template(), "mode", "traditional");
        let cfg: Config = toml::from_str(&out).expect("替换后应仍能解析");
        assert_eq!(cfg.mode, Mode::Traditional);
        // 模板里的注释是这份配置唯一的说明书，不能丢
        assert!(out.contains("免费档速查"));
        assert!(out.contains("第一语言"));
    }

    /// 默认配置的形态：mode=ai、AI 无提供商、传统四家按定稿顺序
    #[test]
    fn 默认值符合定稿() {
        let cfg = Config::default();
        assert_eq!(cfg.mode, Mode::Ai);
        assert!(cfg.ai.order.is_empty());
        assert_eq!(
            cfg.traditional.order,
            vec!["msedge", "transmart", "mymemory", "google"]
        );
    }

    /// flatten 收集：`[ai.zhipu]` 子表要落进 providers，order 不混进去
    #[test]
    fn flatten_收集_ai_提供商() {
        let src = r#"
mode = "ai"
[ai]
order = ["zhipu"]
[ai.zhipu]
base_url = "https://open.bigmodel.cn/api/paas/v4"
models = ["glm-4.7-flash", "glm-4.5-flash"]
api_key = "k"
thinking = false
"#;
        let cfg: Config = toml::from_str(src).expect("应能解析");
        assert_eq!(cfg.ai.order, vec!["zhipu"]);
        assert_eq!(cfg.ai.providers.len(), 1);
        let p = cfg.ai.providers.get("zhipu").expect("应有 zhipu");
        assert_eq!(p.models, vec!["glm-4.7-flash", "glm-4.5-flash"]);
        assert_eq!(p.base_url, "https://open.bigmodel.cn/api/paas/v4");
        assert_eq!(p.thinking, Some(false));
    }

    /// thinking 缺省 = None（不传参数，服务商默认行为）
    #[test]
    fn thinking_缺省为_none() {
        let src = r#"
[ai.zhipu]
base_url = "https://x/v4"
models = ["m"]
api_key = "k"
"#;
        let cfg: Config = toml::from_str(src).unwrap();
        assert_eq!(cfg.ai.providers.get("zhipu").unwrap().thinking, None);
    }

    /// 只有 [ai.zhipu] 没写 order → order 落默认空，providers 仍收集
    #[test]
    fn ai_段缺_order_不炸() {
        let src = r#"
[ai.zhipu]
base_url = "https://x/v4"
models = ["m"]
api_key = "k"
"#;
        let cfg: Config = toml::from_str(src).expect("应能解析");
        assert!(cfg.ai.order.is_empty());
        assert_eq!(cfg.ai.providers.len(), 1);
    }

    /// Mode 只认两个值。未知值让 Mode 解析失败，
    /// 整份配置随后由 load() 警告并回落默认 —— 宁回落、不猜。
    /// （toml::from_str 只吃完整文档，顶层光秃秃一个值不是合法 TOML，
    /// 所以走 Config 级解析来测。）
    #[test]
    fn mode_只认两个值() {
        let cfg: Config = toml::from_str("mode = \"ai\"").unwrap();
        assert_eq!(cfg.mode, Mode::Ai);
        let cfg: Config = toml::from_str("mode = \"traditional\"").unwrap();
        assert_eq!(cfg.mode, Mode::Traditional);
        // 后端名不是模式名 —— 名字空间已经分开，别混；未知值整体解析失败
        assert!(toml::from_str::<Config>("mode = \"mymemory\"").is_err());
        assert!(toml::from_str::<Config>("mode = \"AI_MODE\"").is_err());
    }

    #[test]
    fn provider_名字白名单() {
        assert!(is_valid_provider_name("zhipu"));
        assert!(is_valid_provider_name("glm-free"));
        assert!(is_valid_provider_name("a1"));
        assert!(!is_valid_provider_name(""));
        assert!(!is_valid_provider_name("Zhipu"));
        assert!(!is_valid_provider_name("有中文"));
        assert!(!is_valid_provider_name("with space"));
        assert!(!is_valid_provider_name("col:on"));
    }

    /// Mode 的两个配置文件形态
    #[test]
    fn mode_序列化形态() {
        assert_eq!(mode_marker(Mode::Ai), "ai");
        assert_eq!(mode_marker(Mode::Traditional), "traditional");
        let cfg: Config = toml::from_str("mode = \"traditional\"").unwrap();
        assert_eq!(cfg.mode, Mode::Traditional);
    }
}
