//! 翻译后端的抽象接口。
//!
//! 这是整个项目的架构支点。所有翻译服务都实现 `Backend` trait，
//! 上层 `Translator` 只依赖这个 trait，不关心底下是 MyMemory 还是 GPT。
//!
//! 好处：
//! - 加新后端 = 新写一个文件实现 trait，不用动其他任何代码
//! - 测试时可以塞个「假后端」进去，不发网络请求
//! - 配置里按名字选后端，运行时动态决定

use crate::error::{PoryError, Result};
use crate::lang::Lang;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

// 声明子模块。三个具体后端都实现下面的 Backend trait。
pub mod ai;
pub mod google;
pub mod mymemory;

/// 所有可用后端的名字。
///
/// `build_backend` 的分派、错误提示、以及 `pory -b <名字>` 的合法性校验
/// 都以它为准 —— 名字列表只有这一处定义，加后端时不会漏改某个提示文案。
pub const KNOWN: [&str; 3] = ["mymemory", "google", "ai"];

/// 这个名字是不是已知后端
pub fn is_known(name: &str) -> bool {
    KNOWN.contains(&name)
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
    /// 只有**能报告检测语种**的后端用得上它（目前是 MyMemory：
    /// 它在响应里给 `detectedLanguage`）。Google / AI 后端忽略此字段 ——
    /// 它们自己就能处理同语种，且不向上汇报检测结果。
    pub to_if_same: Option<Lang>,
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
