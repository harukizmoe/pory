//! 翻译核心调度。
//!
//! 这一层的职责是把「用户给的一段任意长的文本」变成
//! 「一串合法的小请求」，逐个发出去，再把结果拼回来。
//!
//! 为什么不能直接整段扔给后端？
//! - MyMemory 单次限 500 字节，长文本直接被拒
//! - 切分后并行/串行发送，出错时能定位到具体哪一块
//!
//! 切分策略：优先在「段落 → 句子 → 强行截断」三级降级，
//! 尽量保证语义完整，不要把一个句子腰斩。

use crate::backend::{Backend, Request};
use crate::cache::Cache;
use crate::error::{PoryError, Result};
use crate::lang::Lang;

/// 一次翻译任务的全部参数
pub struct TranslateJob {
    /// 待翻译的原始文本
    pub text: String,
    /// 源语言
    pub from: Lang,
    /// 目标语言
    pub to: Lang,
    /// 「源语言恰好就是 `to`」时改用的备用目标（第一语言 / 第二语言互翻）。
    /// 语义见 `backend::Request::to_if_same`。
    pub to_if_same: Option<Lang>,
}

/// 一次翻译的统计信息，供界面显示。
pub struct Stats {
    /// 命中缓存的块数
    pub hits: usize,
    /// 走真实请求的块数
    pub misses: usize,
    /// 本次实际用到的后端名。发生回退时会有多个 ——
    /// 所以 `backends.len() > 1` 本身就是「发生过回退」的判据，
    /// 不需要额外维护一个布尔字段。
    pub backends: Vec<String>,
    /// 翻译过程中攒下的警告文案（如「某后端失败，已回退」、缓存写盘失败）。
    ///
    /// 为什么**只记录、不在这里打印**？
    /// 因为终端里有一行动画占着 stderr 的当前行：消息一旦直接 eprintln，
    /// 就会接在动画行的尾巴上，而下一帧的 `\x1b[2K` 会把它一起擦掉 ——
    /// 那等于静默，恰恰是本项目最反对的。所以这一层只负责**产出事实**，
    /// 「什么时候呈现」交给上层（它知道动画什么时候收尾）。
    pub warnings: Vec<String>,
}

/// 单个块的翻译结果来源
struct Outcome {
    /// 结果来自缓存（而非真实请求）
    from_cache: bool,
    /// 本次**尝试过**的后端链（含失败的和最终成功的）。
    ///
    /// 为什么要记失败的？因为只记成功的那个，用户看到 `· mymemory`
    /// 会以为正常走了 MyMemory，根本不知道 AI 已经失败并触发了回退。
    /// 静默回退是不诚实的 —— 必须让人看见「我原本想用 AI，但它挂了」。
    attempted: Vec<String>,
}

/// 调度器。
///
/// 持有**后端链**（第一个是主后端，其余是保底备选），
/// 以及可选的缓存。对外只暴露 `run`。
pub struct Translator {
    /// 后端链。长度至少为 1。
    backends: Vec<Box<dyn Backend>>,
    /// 缓存。`None` 表示本次不用缓存（`--no-cache`）。
    cache: Option<Cache>,
    /// 是否强制忽略已有缓存（`--refresh`）
    refresh: bool,
    /// 累计命中数
    hits: usize,
    /// 累计未命中数
    misses: usize,
    /// 累计用到的后端
    backends_used: Vec<String>,
    /// 累计的警告文案，由 `stats()` 交给上层打印（原因见 `Stats::warnings`）
    warnings: Vec<String>,
}

impl Translator {
    /// 用后端链构造。第一个元素是主后端，其余是保底。
    pub fn new(backends: Vec<Box<dyn Backend>>) -> Self {
        Self {
            backends,
            cache: None,
            refresh: false,
            hits: 0,
            misses: 0,
            backends_used: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// 启用缓存（链式调用）
    pub fn with_cache(mut self, cache: Cache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// 强制刷新：忽略已有缓存，重新翻译并覆盖
    pub fn with_refresh(mut self, refresh: bool) -> Self {
        self.refresh = refresh;
        self
    }

    /// 取本次的统计信息
    pub fn stats(&self) -> Stats {
        Stats {
            hits: self.hits,
            misses: self.misses,
            backends: self.backends_used.clone(),
            warnings: self.warnings.clone(),
        }
    }

    /// 执行翻译，返回完整译文。
    pub async fn run(&mut self, job: &TranslateJob) -> Result<String> {
        let text = job.text.trim();
        if text.is_empty() {
            return Err(PoryError::Input("待翻译文本为空".into()));
        }

        // 切分粒度取后端链里**最保守**的那个上限。
        // 因为切分发生在「还不知道会用哪个后端」的阶段，
        // 按最大的切可能导致回退后端吃不下。
        let max_chars = self
            .backends
            .iter()
            .map(|b| b.max_chars())
            .min()
            .unwrap_or(400);

        let chunks = split_text(text, max_chars);

        let mut results = Vec::with_capacity(chunks.len());

        // 串行发送。为什么不用并发？免费接口对并发很敏感，
        // 同时发 5 个请求很容易触发限流。串行慢一点但稳。
        for chunk in &chunks {
            let (out, outcome) = self
                .translate_one(chunk, &job.from, &job.to, &job.to_if_same)
                .await?;

            // 累计统计
            if outcome.from_cache {
                self.hits += 1;
            } else {
                self.misses += 1;
            }
            // 累计「尝试过的后端」而非只有成功的那个 ——
            // 这样发生回退时，界面能显示完整路径（如 ai → mymemory）
            for name in &outcome.attempted {
                if !self.backends_used.contains(name) {
                    self.backends_used.push(name.clone());
                }
            }

            results.push(out);
        }

        // 收尾：把缓存落盘。失败不影响本次翻译结果，只是记一条警告。
        if let Some(c) = self.cache.as_ref() {
            if let Err(e) = c.save() {
                self.warnings
                    .push(format!("缓存保存失败（不影响本次结果）：{e}"));
            }
        }

        Ok(results.join("\n"))
    }

    /// 翻译单个块，沿后端链依次尝试。
    ///
    /// 这是「保底」机制的核心：
    /// 1. 先用主后端（链首）—— 先查它的缓存，未命中就发请求
    /// 2. 主后端失败 → 自动试下一个后端，直到成功或用尽
    ///
    /// **缓存键跟随实际成功的后端**，这点很关键：
    /// 若 AI 失败、MyMemory 出了结果却存在 AI 的键下，
    /// 下次 AI 恢复时就会命中这个低质量译文 —— 那是脏数据。
    ///
    /// 为什么是**方法**而不是自由函数？它要的东西（后端链、缓存、警告、
    /// refresh 开关）全都是 `Translator` 自己的字段，做成方法就不用一个个
    /// 当参数传（顺带避开 clippy 的 `too_many_arguments`）。
    /// 方法体内 `self.backends`（只读）与 `self.cache` / `self.warnings`（可写）
    /// 是不同字段，Rust 允许同时借用，不冲突。
    async fn translate_one(
        &mut self,
        chunk: &str,
        from: &Lang,
        to: &Lang,
        to_if_same: &Option<Lang>,
    ) -> Result<(String, Outcome)> {
        let mut last_err: Option<PoryError> = None;
        // 记录尝试过的后端链，用于向用户展示真实的回退路径
        let mut attempted: Vec<String> = Vec::new();

        for (idx, backend) in self.backends.iter().enumerate() {
            attempted.push(backend.name().to_string());

            // 缓存键包含后端名 + 后端附加标识（如 AI 的模型名）
            let key = crate::cache::cache_key(
                backend.name(),
                &backend.cache_detail(),
                from.code(),
                to.code(),
                // 备用目标也要进键：开/关互翻时同一个请求会得到不同译文，
                // 见 cache_key 的说明
                to_if_same.as_ref().map(|l| l.code()).unwrap_or(""),
                chunk,
            );

            // ── 查缓存 ──
            // refresh 模式下跳过查询，强制走真实请求。
            if !self.refresh {
                if let Some(hit) = self.cache.as_ref().and_then(|c| c.get(&key)) {
                    return Ok((
                        hit,
                        Outcome {
                            from_cache: true,
                            attempted: attempted.clone(),
                        },
                    ));
                }
            }

            // ── 未命中：调这个后端 ──
            let req = Request {
                text: chunk.to_string(),
                from: from.clone(),
                to: to.clone(),
                to_if_same: to_if_same.clone(),
            };

            match backend.translate(req).await {
                Ok(out) => {
                    // 写缓存（用这个后端自己的键）
                    if let Some(c) = self.cache.as_mut() {
                        c.insert(key, out.clone());
                    }
                    return Ok((
                        out,
                        Outcome {
                            from_cache: false,
                            attempted: attempted.clone(),
                        },
                    ));
                }
                Err(e) => {
                    // 还有备选就记一条警告；已是最后一个则静默
                    // （错误最终会返回给用户，不必重复说）
                    if let Some(next) = self.backends.get(idx + 1) {
                        let msg =
                            format!("{} 失败，回退到 {}：{e}", backend.name(), next.name());
                        self.warnings.push(msg);
                    }
                    last_err = Some(e);
                }
            }
        }

        // 走到这里说明所有后端都失败了
        Err(last_err.unwrap_or_else(|| PoryError::Backend("没有可用的翻译后端".into())))
    }
}

/// 把长文本切分成不超过 `limit` 字符的块。
///
/// 三级策略：
/// 1. 先按空行（段落）切，段落不超过 limit 就独立成块
/// 2. 段落超长，再按句末标点切
/// 3. 还超长，只能硬切（这种情况很少见，通常是没标点的长串）
fn split_text(text: &str, limit: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();

    // 第一级：按段落（连续空行）切
    for para in text.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }

        if char_len(para) <= limit {
            // 段落本身够短，直接成块
            chunks.push(para.to_string());
        } else {
            // 第二级：段落太长，按句子切
            chunks.extend(split_sentences(para, limit));
        }
    }

    // 全都空的话，兜底把原文作为一块（防止空输入导致丢内容）
    if chunks.is_empty() {
        chunks.push(text.to_string());
    }

    chunks
}

/// 按句末标点切分，超长句再硬切
fn split_sentences(para: &str, limit: usize) -> Vec<String> {
    // 中英文的句末标点都认
    const SENTENCE_ENDS: [char; 6] = ['。', '！', '？', '.', '!', '?'];

    let mut out = Vec::new();
    let mut current = String::new();

    for ch in para.chars() {
        current.push(ch);

        // 遇到句末标点且当前积累够长，就切一刀
        if SENTENCE_ENDS.contains(&ch) && char_len(&current) >= limit / 2 {
            out.push(current.trim().to_string());
            current.clear();
        }

        // 当前块已经到上限，不管有没有标点都必须切（第三级：硬切）
        if char_len(&current) >= limit {
            out.push(current.trim().to_string());
            current.clear();
        }
    }

    // 收尾：把没凑满的尾巴也带上
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }

    out.retain(|s| !s.is_empty());
    out
}

/// 按「字符数」而非字节数计算长度。
///
/// 这点很关键：MyMemory 限的是 500 **字节**，一个中文汉字占 3 字节。
/// 但直接用字节数切会把一个汉字切一半（UTF-8 乱码）。
/// 所以按字符数控制，再留足余量来保护字节限制。
fn char_len(s: &str) -> usize {
    s.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 短文本不切分() {
        let chunks = split_text("你好世界", 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "你好世界");
    }

    #[test]
    fn 按段落切分() {
        let text = "第一段。\n\n第二段。\n\n第三段。";
        let chunks = split_text(text, 10);
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn 超长段落按句子切() {
        let long = "这是第一句话。这是第二句话。这是第三句话。这是第四句话。";
        let chunks = split_text(long, 12);
        // 应该被切成多块，且每块不超过上限
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(char_len(c) <= 12, "块超长了：{c}");
        }
    }

    #[test]
    fn 不带标点的长串也能切() {
        let s = "a".repeat(50);
        let chunks = split_text(&s, 10);
        assert!(chunks.len() >= 5);
        for c in &chunks {
            assert!(char_len(c) <= 10);
        }
    }

    #[test]
    fn 切分不丢内容() {
        let text = "abc\ndef\n\nghi";
        let chunks = split_text(text, 100);
        let joined: String = chunks.join("");
        // 所有原始字符都应该还在（顺序可能因 trim 略有变化，但字符集完整）
        for ch in text.chars().filter(|c| !c.is_whitespace()) {
            assert!(joined.contains(ch), "丢了字符 {ch}");
        }
    }
}
