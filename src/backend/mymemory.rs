//! MyMemory 翻译后端。
//!
//! 选它做默认，理由很实在：**不需要 API Key，国内直连可用**。
//! 这对一个刚起步的工具太重要了 —— 你不需要先去注册、充值、配代理，
//! `git clone` 完就能跑。
//!
//! 代价是：
//! - 每日 1000 次请求上限（个人用足够）
//! - 单次最多 500 字节
//! - 质量不如 DeepL / AI，但日常够用
//! - **它的 Autodetect 通道有两个坑**（会把原文退回、会用空译文表示「不用翻」），
//!   对策见 `do_translate` 里的注释
//!
//! 接口文档：https://mymemory.translated.net/doc/spec.php

use crate::backend::{http_client, net_err, same_primary_language, Backend, Request};
use crate::error::{PoryError, Result};

const ENDPOINT: &str = "https://api.mymemory.translated.net/get";

/// MyMemory 后端。目前不含状态，但保留结构体以便将来加缓存或 key 字段。
pub struct MyMemory {
    /// 可选的联系邮箱。填了能提高每日额度（官方推荐做法）。
    email: Option<String>,
}

/// 一次请求的解析结果。
///
/// 比只返回一个 `String` 多带两个信息，`do_translate` 的两个分支全靠它们：
/// - `text` 为 `None` ⇒ 服务端认为**不必翻译**（源语言就是目标语言）
/// - `detected` 有值 ⇒ 服务端汇报了检测语种，是「退回原文」时的补救依据
struct Reply {
    /// 译文。服务端认为不必翻时是 `None`。
    text: Option<String>,
    /// 服务端自报的检测语种（如 `en`）。显式指定源语言时不会返回这个字段。
    detected: Option<String>,
}

impl MyMemory {
    /// 构造后端。`email` 来自配置文件，没配就传 None。
    pub fn new(email: Option<String>) -> Self {
        Self { email }
    }
}

impl Backend for MyMemory {
    fn name(&self) -> &str {
        "mymemory"
    }

    fn max_chars(&self) -> usize {
        // 官方限制是 500 字节。中文一个字 3 字节，所以实际能翻的中文更少。
        // 这里按字符数给个保守值，真正的字节数检查在切分时做。
        400
    }

    // 注意：签名与 trait 定义一致，返回装箱后的 Future。
    // 函数体是普通 async fn 写法，Box::pin 负责包装成 trait 对象。
    fn translate<'a>(
        &'a self,
        req: Request,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(self.do_translate(req))
    }
}

// 实际逻辑放在这个 inherent 方法里，保持 async 语法清爽。
// 上面的 trait 方法只做一层装箱转发。
impl MyMemory {
    /// 发一次请求并解析。
    ///
    /// `from` 传的是**已经转换好**的 MyMemory 语言码（可能是 `Autodetect`）。
    /// 「该用哪个源语言」的判断全部留在 `do_translate`，这里只管收发 ——
    /// 重试逻辑才能在一个地方读明白。
    async fn request(&self, text: &str, from: &str, to: &str) -> Result<Reply> {
        // 组装查询参数。urlencoding 会自动处理中文和特殊字符。
        let mut url = format!(
            "{ENDPOINT}?q={}&langpair={}|{}",
            urlencoding::encode(text),
            urlencoding::encode(from),
            urlencoding::encode(to),
        );

        // 带上邮箱能提额度（官方推荐）
        if let Some(email) = &self.email {
            url.push_str(&format!("&de={}", urlencoding::encode(email)));
        }

        // 发 GET 请求，解析成 JSON。
        // 经统一的构造函数发请求，才有超时兜底（见 backend/mod.rs 的说明）。
        let resp = http_client()?.get(&url).send().await.map_err(net_err)?;

        // HTTP 状态码检查
        if !resp.status().is_success() {
            return Err(PoryError::Backend(format!(
                "MyMemory 返回 HTTP {}",
                resp.status()
            )));
        }

        // 解析响应体
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| PoryError::Parse(format!("MyMemory 响应不是合法 JSON：{e}")))?;

        // MyMemory 的结构：{ "responseData": { "translatedText": "..." }, "responseStatus": 200 }
        let status = body["responseStatus"].as_i64().unwrap_or(0);
        if status != 200 {
            let msg = body["responseDetails"]
                .as_str()
                .unwrap_or("未知错误")
                .to_string();
            return Err(PoryError::Backend(format!(
                "MyMemory 拒绝了请求（status={status}）：{msg}"
            )));
        }

        // 注意 translatedText 可能是 null（见 do_translate 的情况一），
        // 所以这里不能当它是字符串就直接取。
        Ok(Reply {
            text: body["responseData"]["translatedText"]
                .as_str()
                .map(|s| s.to_string()),
            detected: body["responseData"]["detectedLanguage"]
                .as_str()
                .map(|s| s.to_string()),
        })
    }

    /// 翻译一块文本。
    async fn do_translate(&self, req: Request) -> Result<String> {
        let to = req.to.to_mymemory();

        // 源语言是明确的：本地就能判掉「同语种」这一种，剩下的才发请求。
        if !req.from.is_auto() {
            // 为什么要在本地判：`pory "你好" -f zh` 就会撞上 —— 默认目标语言就是 zh。
            // 而 MyMemory 对这种请求的回应很难看：`zh|zh` 会返回
            // responseStatus=0 + 一句英文的 `PLEASE SELECT TWO DISTINCT LANGUAGES`，
            // 透传给用户毫无意义。本地判断既准确又省一次请求。
            //
            // 比的是「主语言」：MyMemory 只认两位码，所以拆两种说法 ——
            // - 完整语言码相同 → 真的无需翻译
            // - 仅主语言相同（zh-CN 对 zh-TW）→ 它区分不了这对，如实说明
            if same_primary_language(req.from.code(), req.to.code()) {
                if req.from.code() == req.to.code() {
                    eprintln!("⚠ 源语言与目标语言相同（{}），原样返回", req.from.code());
                } else {
                    eprintln!(
                        "⚠ MyMemory 只认两位主语言码，区分不了 {} 与 {}，原样返回",
                        req.from.code(),
                        req.to.code()
                    );
                }
                return Ok(req.text.clone());
            }

            let Reply { text, .. } = self
                .request(&req.text, &req.from.to_mymemory(), &to)
                .await?;
            return required_text(text);
        }

        // 源语言待检测：先让 MyMemory 自己认。
        // 注意它不认 "auto" 这个写法，要叫 "Autodetect"。
        let Reply { text, detected } = self.request(&req.text, "Autodetect", &to).await?;

        // ── 情况一：服务端表示「不用翻」 ──
        // 实测（2026-09-20）：检测出的源语言就是目标语言时，
        // translatedText 是 null，而 detectedLanguage 有值：
        //   q=你好世界 & langpair=Autodetect|zh
        //   → {"translatedText": null, "detectedLanguage": "zh-CN"}
        //
        // 此时「译文」确实就是原文。但更该做的是**换方向** ——
        // 原文已经是我要的语言了，那就翻成另一门语言（第一语言 / 第二语言互翻），
        // 这才是 auto 该有的样子；没有备用目标时才原样返回。
        // （之前这里被当成解析失败报「找不到 translatedText」，那是误导。）
        if text.is_none() {
            if let Some(detected) = &detected {
                if same_primary_language(detected, &to) {
                    // A. 原文就是目标语言 → 有备用目标就改译它
                    if let Some(alt) = &req.to_if_same {
                        // 只有这条路径会多花一次请求；正常翻译一次到位。
                        // 换方向仍拿不到译文时退回原文，不再纠缠。
                        let Reply { text: swapped, .. } = self
                            .request(&req.text, "Autodetect", &alt.to_mymemory())
                            .await?;
                        return Ok(swapped.unwrap_or_else(|| req.text.clone()));
                    }

                    // B. 没配备用目标 → 原文即译文，原样返回。
                    // 提示写到 stderr：译文走 stdout 是硬约定，别污染管道输出。
                    eprintln!("⚠ 原文已经是目标语言（{detected}），原样返回");
                    return Ok(req.text.clone());
                }

                // C. 检测语种与目标语言不同、却没给译文：如实说明，原样返回
                eprintln!("⚠ MyMemory 未给出译文（检测为 {detected}），原样返回原文");
                return Ok(req.text.clone());
            }
            return Err(PoryError::Parse(
                "MyMemory 既没返回译文，也没返回检测语种".into(),
            ));
        }

        let translated = required_text(text)?;

        // ── 情况二：Autodetect 把原文原样退回 ──
        // 它的「自动检测」走 MyMemory 自家的机器翻译，偶尔**没译出来就直接退回原文**：
        //   Autodetect|zh-CN   "hello world" → "hello world"（match 0.85，一句没译）
        //   en|zh-CN           "hello world" → "你好世界"    （命中人工记忆库，match 1.0）
        // 好在响应里顺带汇报了 detectedLanguage，用它显式再要一次就能补上。
        //
        // 只在「译文恰好等于原文」这条失败路径上重试 —— 正常请求不会多一次往返。
        // 检测语种缺失、或检测结果本就是目标语言时不重试
        //（后者是情况一该管的事，退到这里说明不必再试）。
        if translated == req.text {
            if let Some(detected) = detected.as_deref().filter(|d| !d.is_empty()) {
                if !same_primary_language(detected, &to) {
                    // 重试也可能同样退回原文，那就如实返回原文，不再纠缠
                    let Reply { text, .. } = self.request(&req.text, detected, &to).await?;
                    return required_text(text);
                }
            }
        }

        Ok(translated)
    }
}

/// 取出必须存在的译文。
///
/// 「服务端没给译文」在 `do_translate` 的情况一里是正常现象（已提前返回），
/// 走到这里还没译文才是真异常。
fn required_text(text: Option<String>) -> Result<String> {
    text.ok_or_else(|| PoryError::Parse("MyMemory 没返回译文（响应里没有 translatedText）".into()))
}

#[cfg(test)]
mod tests {
    // same_primary_language 已提到 backend/mod.rs 共用（三个传统后端都要它），
    // 它的测试在那里；这里只留 MyMemory 自己的行为测试。

    #[test]
    fn 主语言码比较在共用模块里可用() {
        // 顺手钉住「提出去之后仍然可达、语义不变」——这是重构的安全绳
        use crate::backend::same_primary_language;
        assert!(same_primary_language("zh-CN", "zh-Hans"));
        assert!(same_primary_language("en", "en-GB"));
        assert!(!same_primary_language("zh", "en"));
        assert!(!same_primary_language("", "zh-CN"));
    }
}
