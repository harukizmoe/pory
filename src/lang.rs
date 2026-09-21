//! 语言代码的统一处理。
//!
//! 各家翻译后端的语言代码写法不一致：
//! - Google 用 `zh-CN`、`ja`
//! - MyMemory 用 `zh`、`ja`
//! - AI 后端更喜欢自然语言描述（"简体中文"）
//!
//! 与其在每个后端里各写一套判断，不如在这里统一成 `Lang` 类型，
//! 由各后端自己决定怎么转成对方要的格式。

use crate::error::{PoryError, Result};

/// 语言标签。本质上是一个规范化后的字符串。
///
/// 为什么用结构体包一层而不是直接用 String？
/// 因为「非法语种」这个错误应该在构造时就被拦住，
/// 而不是等到把请求发出去、后端报错了才发现。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lang(String);

/// 目前支持的语言。键是用户输入的简写，值是规范化的 BCP-47 风格代码。
///
/// 这张表可以随时扩。挑的都是常用语种 + 翻译质量较好的。
const SUPPORTED: &[(&str, &str)] = &[
    ("zh", "zh-CN"),   // 简体中文
    ("zh-cn", "zh-CN"),
    ("zh-tw", "zh-TW"), // 繁体中文
    ("en", "en"),
    ("ja", "ja"),
    ("ko", "ko"),
    ("fr", "fr"),
    ("de", "de"),
    ("es", "es"),
    ("ru", "ru"),
    ("it", "it"),
    ("pt", "pt"),
    ("ar", "ar"),
    ("th", "th"),
    ("vi", "vi"),
];

/// 所有可用的语种别名（`--shell` 生成的补全脚本用）。
///
/// 与 `SUPPORTED` 同源 —— 补全候选和解析器认的别名永远不会漂移。
pub fn aliases() -> Vec<&'static str> {
    SUPPORTED.iter().map(|(a, _)| *a).collect()
}

impl Lang {
    /// 从用户输入构造语言标签。
    ///
    /// 大小写不敏感，`ZH-CN` 和 `zh-cn` 都认。
    /// 特殊值 `auto` 用于「让后端自动检测源语言」。
    pub fn parse(input: &str) -> Result<Self> {
        let lower = input.trim().to_lowercase();

        // auto 是保留值，表示不指定源语言
        if lower == "auto" {
            return Ok(Lang("auto".to_string()));
        }

        // 查表匹配
        for (alias, canonical) in SUPPORTED {
            if *alias == lower {
                return Ok(Lang(canonical.to_string()));
            }
        }

        Err(PoryError::Input(format!(
            "不支持的语种 `{input}`。可用的有：{}",
            SUPPORTED
                .iter()
                .map(|(a, _)| *a)
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }

    /// 源语言是否待自动检测
    pub fn is_auto(&self) -> bool {
        self.0 == "auto"
    }

    /// 取出规范化的代码字符串
    pub fn code(&self) -> &str {
        &self.0
    }

    /// 转换成 MyMemory 需要的格式：它只要两位主语言码，`zh-CN` 要变成 `zh`
    pub fn to_mymemory(&self) -> String {
        // 按 '-' 切分取第一段，再转小写
        self.0
            .split('-')
            .next()
            .unwrap_or(&self.0)
            .to_lowercase()
    }

    /// 转换成微软翻译（Edge 端点）的格式。
    ///
    /// 微软用 `zh-Hans` / `zh-Hant` 区分简繁，而不是 BCP-47 风格的 `zh-CN` /
    /// `zh-TW`；其余语种两家写法一致，原样透传。
    /// auto 不经过这里 —— 调用处对 auto 的处理是「干脆不带 from 参数」，
    /// 让微软自己检测（实测可行，见 msedge.rs）。
    pub fn to_microsoft(&self) -> String {
        match self.0.as_str() {
            "zh-CN" => "zh-Hans".to_string(),
            "zh-TW" => "zh-Hant".to_string(),
            other => other.to_string(),
        }
    }

    /// 转换成自然语言名称，给 AI 后端写 prompt 用
    pub fn to_name(&self) -> &str {
        match self.0.as_str() {
            "zh-CN" => "简体中文",
            "zh-TW" => "繁体中文",
            "en" => "英语",
            "ja" => "日语",
            "ko" => "韩语",
            "fr" => "法语",
            "de" => "德语",
            "es" => "西班牙语",
            "ru" => "俄语",
            "it" => "意大利语",
            "pt" => "葡萄牙语",
            "ar" => "阿拉伯语",
            "th" => "泰语",
            "vi" => "越南语",
            "auto" => "自动检测",
            other => other,
        }
    }
}

impl std::fmt::Display for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 识别常见语种() {
        assert_eq!(Lang::parse("zh").unwrap().code(), "zh-CN");
        assert_eq!(Lang::parse("ZH-CN").unwrap().code(), "zh-CN");
        assert_eq!(Lang::parse(" ja ").unwrap().code(), "ja");
    }

    #[test]
    fn auto_是合法值() {
        let l = Lang::parse("auto").unwrap();
        assert!(l.is_auto());
    }

    #[test]
    fn 拒绝不支持的语种() {
        assert!(Lang::parse("klingon").is_err());
    }

    #[test]
    fn 转成_mymemory_格式时砍掉区域码() {
        assert_eq!(Lang::parse("zh-CN").unwrap().to_mymemory(), "zh");
        assert_eq!(Lang::parse("ja").unwrap().to_mymemory(), "ja");
    }

    #[test]
    fn 转成_microsoft_格式时换简繁写法() {
        assert_eq!(Lang::parse("zh-CN").unwrap().to_microsoft(), "zh-Hans");
        assert_eq!(Lang::parse("zh-TW").unwrap().to_microsoft(), "zh-Hant");
        assert_eq!(Lang::parse("ja").unwrap().to_microsoft(), "ja");
        assert_eq!(Lang::parse("en").unwrap().to_microsoft(), "en");
    }
}
