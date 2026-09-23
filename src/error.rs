//! 统一的错误类型。
//!
//! Rust 里错误处理是显式的：函数返回 `Result<T, E>`，调用方必须处理。
//! 这里用 `thiserror` 定义一个覆盖全项目的错误枚举，好处是：
//! 1. 每个错误都有明确的语义，不用靠字符串猜
//! 2. `?` 运算符可以自动把底层错误（网络错误、JSON 错误）转换成我们的类型
//! 3. 最终在 main 里统一打印成人类可读的提示

use thiserror::Error;

/// pory 可能遇到的所有错误
#[derive(Error, Debug)]
pub enum PoryError {
    /// 网络请求失败（连不上、超时、DNS 解析不了）。
    ///
    /// 存 `String` 而不是 `reqwest::Error`，是为了**能改写成用户看得懂的话** ——
    /// 超时需要单独说清楚，而 reqwest 自己的文案
    /// （`error sending request for url(...)`）完全看不出是超时。
    /// 转换统一走 `backend::net_err()`，那里才认识 reqwest 的类型。
    #[error("网络请求失败：{0}")]
    Network(String),

    /// 整次翻译超过了总时限。
    #[error("翻译超时：整次调用超过 {0} 秒上限。可用 --timeout 调大上限")]
    TranslationTimeout(u64),

    /// 词典 AI 候选链超过本次查词的总时限。
    #[error("查词超时：整次调用超过 {0} 秒上限。可用 --timeout 调大上限")]
    DictionaryTimeout(u64),

    /// 后端返回的数据格式不符合预期
    #[error("解析后端响应失败：{0}")]
    Parse(String),

    /// 后端明确返回了错误（比如额度用尽、语种不支持）
    #[error("翻译后端报错：{0}")]
    Backend(String),

    /// 配置文件读取或解析出错
    #[error("配置错误：{0}")]
    Config(String),

    /// 输入不合法（比如空文本）
    #[error("输入错误：{0}")]
    Input(String),

    /// 缓存读写出错。
    ///
    /// 注意：缓存问题**不应该阻断翻译**。这个变体只用于
    /// 「缓存自身确实坏了」的场景（写盘失败等），
    /// 而「缓存文件不存在 / 解析失败」会静默降级为空缓存。
    #[error("缓存错误：{0}")]
    Cache(String),
}

/// 项目统一的 Result 别名。
///
/// 写 `Result<String>` 比写 `Result<String, PoryError>` 短，
/// 而且以后换错误类型时只需要改这一处。
pub type Result<T> = std::result::Result<T, PoryError>;
