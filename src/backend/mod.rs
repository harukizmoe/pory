//! 翻译后端的抽象接口。
//!
//! 这是整个项目的架构支点。所有翻译服务都实现 `Backend` trait，
//! 上层 `Translator` 只依赖这个 trait，不关心底下是 MyMemory 还是 GPT。
//!
//! 好处：
//! - 加新后端 = 新写一个文件实现 trait，不用动其他任何代码
//! - 测试时可以塞个「假后端」进去，不发网络请求
//! - 配置里按名字选后端，运行时动态决定

use crate::config::{Config, TRADITIONAL_KNOWN};
use crate::error::{PoryError, Result};
use crate::lang::Lang;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

// 声明子模块。传统后端各实现下面的 Backend trait；
// AI 提供商由同一个 `Ai` 类型按配置实例化多个（名字随实例走）。
pub mod ai;
pub mod google;
pub mod msedge;
pub mod mymemory;
pub mod transmart;

/// 按名字构造一个**传统**后端（免 Key 机翻）。
///
/// 名单由 `config::TRADITIONAL_KNOWN` 定义，两边必须保持一致 ——
/// 配置解析警告、错误提示、这里的 match 三处都以那份常量为口径。
///
/// 与 AI 提供商不同：传统后端免 Key、恒可构建（只要名字合法），
/// 所以「名字不认识」是唯一的失败方式。
pub fn build_traditional(name: &str, cfg: &Config) -> Result<Box<dyn Backend>> {
    match name {
        "msedge" => Ok(Box::new(msedge::MsEdge::new())),
        "transmart" => Ok(Box::new(transmart::Transmart::new())),
        "mymemory" => Ok(Box::new(mymemory::MyMemory::new(
            cfg.mymemory_email.clone(),
        ))),
        "google" => Ok(Box::new(google::Google::new(cfg.google_endpoint.clone()))),
        other => Err(PoryError::Config(format!(
            "未知的传统后端 `{other}`。可用：{}",
            TRADITIONAL_KNOWN.join(" / ")
        ))),
    }
}

/// 单次 HTTP 请求的超时上限。
///
/// **这不是可选项。** reqwest 的默认 `timeout` 是 `None`，意思是无限等待：
/// 一旦连接挂起（比如出口被墙、DNS 返回了不可达的假 IP），请求会一直等到
/// 内核 TCP 超时 —— Linux 上约 130 秒。用户看到的就是「卡死」，而且
/// 保底后端也轮不上（回退发生在上一个后端**返回失败**之后）。
///
/// 取 30 秒：正常翻译约 1 秒（实测），30 秒是极宽松的上限；
/// 对慢的大模型后端也够用。长文本会被切成多块，**每块各自计时**。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// 构造带超时的 HTTP 客户端。
///
/// **所有后端都必须经由这个函数发请求**，别用 `reqwest::get()` ——
/// 那个便捷函数每次建一个默认配置的临时客户端，`timeout` 为 `None`，
/// 等于没有兜底。（超时集中定义在这里，也免得三个后端各写一遍常量。）
pub fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(net_err)
}

/// 把 reqwest 的错误翻成用户看得懂的话。
///
/// 重点是**超时要说清楚**：reqwest 的 Display 只给
/// `error sending request for url(...)`，从中看不出是超时 ——
/// 而超时恰恰是最需要被认出来的一种失败（意味着网络不通或被墙，
/// 也正好解释了我们为什么给它设了上限）。其余错误保留原文案。
pub fn net_err(e: reqwest::Error) -> PoryError {
    if e.is_timeout() {
        PoryError::Network(format!(
            "请求超时（超过 {} 秒），网络可能不通或被墙。若用 google 后端，国内通常需要代理",
            REQUEST_TIMEOUT.as_secs()
        ))
    } else {
        PoryError::Network(e.to_string())
    }
}

/// 一次翻译请求的参数
#[derive(Debug, Clone)]
pub struct Request {
    /// 待翻译的文本（已经切分好的单块）
    pub text: String,
    /// 源语言，可能是 auto
    pub from: Lang,
    /// 目标语言
    pub to: Lang,
    /// 「源语言恰好就是 `to`」时改用的备用目标。
    ///
    /// 用途是**第一语言 / 第二语言互翻**：auto 模式下请求「翻成中文」，
    /// 而原文本来就是中文时，改译成另一门语言 —— 而不是把原文原样吐回去。
    ///
    /// `None` 表示没有备用目标，此时后端遇到这种情况就原样返回原文。
    ///
    /// **每个后端都必须自己处理这个字段**（2026-09-22 修正了一个错误认知：
    /// 早先这里写着「Google / AI 自己就能处理同语种」，实测证明是错的 ——
    /// 免 Key 的端点全都老实执行「把中文翻成中文」，也就是原样返回原文）。
    /// 证据：
    /// - MyMemory：用服务端给的 `translatedText=null` + `detectedLanguage` 判断
    /// - AI：把「若原文已是 X 则改译 Y」写进 prompt
    /// - msedge / transmart / google：**不换向**，得靠 `looks_untranslated`
    ///   或响应里的检测语种自己判断（见 `same_language_handling`）
    pub to_if_same: Option<Lang>,
}

/// 判定这块文本「没有真正被翻译」—— 译文与原文完全相同（忽略首尾空白）。
///
/// 用途：auto 模式下原文已经是母语时，免 Key 的传统端点普遍**不会自己换向**，
/// 而是把原文当译文退回（2026-09-22 实测 msedge / transmart 都是这样）。
/// 这是本项目最不希望出现的失败形态：静默、且结果看着像成功。
///
/// 这是启发式（「OK」翻成「OK」也会命中），但两种结局都无害：
/// 有备用目标就换向重发（多一次请求、结果更对），没有就原样返回（结果本来就对）。
pub(crate) fn looks_untranslated(translated: &str, source: &str) -> bool {
    translated.trim() == source.trim()
}

/// 比较两个语言码的「主语言」部分，忽略区域码与大小写：
/// `zh-CN` 与 `zh-TW`、`en` 与 `en-GB`、`zh-Hans` 与 `zh` 各算同一门语言。
///
/// 用途：判断「检测出的源语言是不是就是目标语言」—— 是的话说明原文已是母语，
/// 该换向而不是再翻一次自己。
pub(crate) fn same_primary_language(a: &str, b: &str) -> bool {
    /// 取 `zh-CN` / `zh_TW` / `zh-Hans` 里 `-` 或 `_` 之前的主语言码
    fn primary(code: &str) -> &str {
        code.split(['-', '_']).next().unwrap_or("")
    }
    let (pa, pb) = (primary(a), primary(b));
    !pa.is_empty() && pa.eq_ignore_ascii_case(pb)
}

/// 翻译后端必须实现的行为。
///
/// 关于返回类型为什么要写成 `Pin<Box<dyn Future...>>` 而不是 `async fn`：
/// Rust 1.75 起支持 trait 里直接写 `async fn`，但那样的 trait **不能**
/// 当 `dyn Backend` 用（因为每个实现的 Future 类型不同，没法建虚表）。
/// 我们需要在运行时按配置选后端，所以必须手动把 Future 装箱成
/// trait 对象。这是目前的标准做法，代价是每次调用多一次堆分配 ——
/// 对翻译这种本来就是网络 IO 的场景完全无所谓。
pub trait Backend: Send + Sync {
    /// 后端的人类可读名字，出现在提示和错误信息里
    fn name(&self) -> &str;

    /// 影响翻译结果的附加配置标识，用于构成缓存键。
    ///
    /// 比如 AI 后端要返回模型名 —— 否则从 gpt-4o 换成 deepseek-chat
    /// 后会命中旧模型的缓存，拿到风格完全不同的译文。
    /// 对配置单一的后端（MyMemory / Google）返回空串即可。
    fn cache_detail(&self) -> String {
        String::new()
    }

    /// 这个后端单次请求能接受的最大字符数。
    ///
    /// 超长文本会被 `Translator` 按这个上限切分。
    /// MyMemory 是 400，Google 是 4000，AI 可以到 8000。
    fn max_chars(&self) -> usize;

    /// 执行翻译。输入一块文本，返回译文。
    fn translate<'a>(
        &'a self,
        req: Request,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;
}

#[cfg(test)]
mod tests {
    use super::{looks_untranslated, same_primary_language};

    #[test]
    fn 主语言码比较忽略区域码与大小写() {
        assert!(same_primary_language("zh-CN", "zh-TW"));
        assert!(same_primary_language("zh", "zh-Hans"));
        assert!(same_primary_language("en", "en-GB"));
        assert!(same_primary_language("EN", "en"));
        assert!(!same_primary_language("en", "zh-CN"));
        assert!(!same_primary_language("ja", "zh"));
    }

    /// 空串不与任何语言相同 —— 否则换向逻辑会拼出空目标码，把「没翻译」变成请求报错
    #[test]
    fn 空语言码不与任何语言相同() {
        assert!(!same_primary_language("", "zh-CN"));
        assert!(!same_primary_language("", ""));
        assert!(!same_primary_language("zh", ""));
    }

    /// 「译文 == 原文」判定：忽略首尾空白，其余一律严格比较
    #[test]
    fn 未翻译判定() {
        assert!(looks_untranslated("今天天气不错", "今天天气不错"));
        assert!(looks_untranslated("  今天天气不错\n", "今天天气不错"));
        assert!(!looks_untranslated("The weather is nice", "今天天气不错"));
        // 只差一个字也算翻译过 —— 宁可漏判也不误判（误判会多花一次换向请求）
        assert!(!looks_untranslated("今天天气不错啊", "今天天气不错"));
    }
}
