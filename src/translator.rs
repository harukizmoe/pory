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
use std::time::Duration;

/// 整次翻译的默认墙钟上限。单个后端请求仍受各自的 30 秒超时约束。
pub const DEFAULT_TRANSLATION_TIMEOUT: Duration = Duration::from_secs(300);

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
///
/// 计数单位是**实际请求单元**：回退到上限更小的后端时，一个用户可见的块
/// 会再切成多个单元，每个单元各算一次命中或未命中 —— 这比按大块计数
/// 更贴近真实开销，脚注里的 block 数也才对得上发出去的请求数。
#[derive(Default)]
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
    /// 整次翻译的墙钟耗时（含切块、全部块的请求/缓存、回退尝试与缓存落盘）。
    ///
    /// 计时在 `Translator::run` 内部完成 —— 它是翻译过程本身，比外层
    /// （main 的动画包装）量得更准；缓存命中显示 `0.0s` 是如实报告
    /// 「瞬时」，与动画「命中不闪帧」同一哲学。
    pub duration: std::time::Duration,
}

/// 一次 `Translator::run` 的完整产出：结果与统计。
///
/// 失败时统计同样有意义（试过哪些后端、攒了哪些警告），所以两者一起返回，
/// 而不是写成 `Result<...>` 把统计丢掉 —— 调用方无论成败都要能打出警告。
pub struct Report {
    pub outcome: Result<String>,
    pub stats: Stats,
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
    /// 所有文本块与回退请求共享的整次调用时限。
    timeout: Duration,
}

impl Translator {
    /// 用后端链构造。第一个元素是主后端，其余是保底。
    pub fn new(backends: Vec<Box<dyn Backend>>) -> Self {
        Self {
            backends,
            cache: None,
            refresh: false,
            timeout: DEFAULT_TRANSLATION_TIMEOUT,
        }
    }

    /// 设置整次翻译的墙钟上限。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
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

    /// 执行翻译，返回完整译文与本次统计。
    ///
    /// 统计随结果一起返回（见 `Report`）—— 对象本身只保存配置，
    /// 跑两次不会互相累加，也不存在「忘了先清零」这种状态。
    pub async fn run(&mut self, job: &TranslateJob) -> Report {
        let text = job.text.trim();
        if text.is_empty() {
            return Report {
                outcome: Err(PoryError::Input("待翻译文本为空".into())),
                stats: Stats::default(),
            };
        }

        // 墙钟计时从切块前开始（切块也是翻译过程的一部分）
        let start = std::time::Instant::now();
        let deadline = tokio::time::Instant::now() + self.timeout;
        let mut stats = Stats::default();

        // 切分粒度取**主后端**的上限。回退到上限更小的后端时，translate_one
        // 会按那个后端自己的上限再切一次 —— 否则 MyMemory 的 400 字符上限
        // 会替 AI 决定切块，长段落永远进不了大模型的上下文。
        let max_chars = self.backends.first().map(|b| b.max_chars()).unwrap_or(400);

        let split = split_text(text, max_chars);
        let mut output = String::new();
        let mut timed_out = false;

        // 串行发送。为什么不用并发？免费接口对并发很敏感，
        // 同时发 5 个请求很容易触发限流。串行慢一点但稳。
        for piece in &split.pieces {
            let translated = tokio::time::timeout_at(
                deadline,
                self.translate_one(&piece.text, &job.from, &job.to, &job.to_if_same, &mut stats),
            )
            .await;
            let out = match translated {
                Ok(Ok(result)) => result,
                // 失败也要把已经攒下的统计与警告交还给调用方
                Ok(Err(error)) => {
                    stats.duration = start.elapsed();
                    return Report {
                        outcome: Err(error),
                        stats,
                    };
                }
                Err(_) => {
                    timed_out = true;
                    break;
                }
            };

            // 块间空白用原文的，不另外插入换行：段落空行、缩进、英文句间空格
            // 都是原文结构的一部分，拼回去必须一模一样。
            output.push_str(&piece.before);
            output.push_str(out.trim());
        }
        output.push_str(&split.tail);

        // 收尾：把缓存落盘。失败不影响本次翻译结果，只是记一条警告。
        if let Some(c) = self.cache.as_ref() {
            if let Err(e) = c.save() {
                stats
                    .warnings
                    .push(format!("缓存保存失败（不影响本次结果）：{e}"));
            }
        }

        // 耗时含缓存落盘 —— 用户看到的「翻译用了多久」应当是完整墙钟时间
        stats.duration = start.elapsed();

        Report {
            outcome: if timed_out {
                Err(PoryError::TranslationTimeout(self.timeout.as_secs()))
            } else {
                Ok(output)
            },
            stats,
        }
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
    /// **块大小按当前后端自己的上限决定**：上层按主后端上限切块，
    /// 轮到上限更小的后端时在这里再切一次。切分保留块间空白，
    /// 所以二次切分不会改变译文结构。
    ///
    /// 警告与计数都写进 `stats`，而不是存进 `self`：它们属于某一次 run，
    /// 不属于调度器。失败路径也必须把已经攒下的统计交出去。
    async fn translate_one(
        &mut self,
        chunk: &str,
        from: &Lang,
        to: &Lang,
        to_if_same: &Option<Lang>,
        stats: &mut Stats,
    ) -> Result<String> {
        // 备用目标也要进键：开/关互翻时同一个请求会得到不同译文，
        // 见 cache_key 的说明
        let alt = to_if_same.as_ref().map(|l| l.code()).unwrap_or("");
        let mut last_err: Option<PoryError> = None;

        for idx in 0..self.backends.len() {
            let (name, detail, max_chars) = {
                let backend = &self.backends[idx];
                (
                    backend.name().to_string(),
                    backend.cache_detail(),
                    backend.max_chars(),
                )
            };
            // 记录**尝试过**的后端，而不只是成功的那个 ——
            // 这样发生回退时，界面能显示完整路径（如 ai → mymemory）。
            // 失败时这条记录同样重要，所以先记再试。
            if !stats.backends.contains(&name) {
                stats.backends.push(name.clone());
            }

            // 这个后端吃不下整块：按它自己的上限再切，块内空白原样保留。
            // 切分只影响本次请求，不改变上层的块边界。
            let split = if char_len(chunk) > max_chars {
                split_text(chunk, max_chars)
            } else {
                Split {
                    pieces: vec![Piece {
                        before: String::new(),
                        text: chunk.to_string(),
                    }],
                    tail: String::new(),
                }
            };

            let mut translated = String::new();
            let mut failure: Option<PoryError> = None;

            for piece in &split.pieces {
                // 缓存键包含后端名 + 后端附加标识（如 AI 的模型名）
                let key = crate::cache::cache_key(
                    &name,
                    &detail,
                    from.code(),
                    to.code(),
                    alt,
                    &piece.text,
                );

                // ── 查缓存 ──
                // refresh 模式下跳过查询，强制走真实请求。
                if !self.refresh {
                    if let Some(hit) = self.cache.as_ref().and_then(|c| c.get(&key)) {
                        stats.hits += 1;
                        translated.push_str(&piece.before);
                        translated.push_str(&hit);
                        continue;
                    }
                }

                // ── 未命中：调这个后端 ──
                let req = Request {
                    text: piece.text.clone(),
                    from: from.clone(),
                    to: to.clone(),
                    to_if_same: to_if_same.clone(),
                };

                match self.backends[idx].translate(req).await {
                    Ok(out) => {
                        stats.misses += 1;
                        // 写缓存（用这个后端自己的键）
                        if let Some(c) = self.cache.as_mut() {
                            c.insert(key, out.clone());
                        }
                        translated.push_str(&piece.before);
                        translated.push_str(&out);
                    }
                    Err(e) => {
                        // 已翻好的子块保留在缓存里，但这一整块算失败 ——
                        // 半块译文交出去就是结构错误，宁可整块走回退。
                        failure = Some(e);
                        break;
                    }
                }
            }

            if let Some(e) = failure {
                // 还有备选就记一条警告；已是最后一个则静默
                // （错误最终会返回给用户，不必重复说）
                if let Some(next) = self.backends.get(idx + 1) {
                    stats
                        .warnings
                        .push(format!("{} 失败，回退到 {}：{e}", name, next.name()));
                }
                last_err = Some(e);
                continue;
            }

            translated.push_str(&split.tail);
            return Ok(translated);
        }

        // 走到这里说明所有后端都失败了
        Err(last_err.unwrap_or_else(|| PoryError::Backend("没有可用的翻译后端".into())))
    }
}

/// 一块待翻译文本及其在原文中的前置空白。
///
/// 为什么记 `before` 而不是只存内容、最后统一用换行拼接：切块必须可逆。
/// 段落之间的空行、块首的缩进、英文句间的空格都是原文结构的一部分；
/// 丢掉它们（或用 `\n` 重新拼）等于悄悄改写译文结构，而 prompt 里
/// 刚刚才承诺「严格保留换行与缩进」。
struct Piece {
    /// 紧挨着本块之前的原始空白（首块则是文档的前导空白）。
    before: String,
    /// 本块内容，两端不含空白。
    text: String,
}

/// `split_text` 的完整结果。
struct Split {
    pieces: Vec<Piece>,
    /// 最后一块之后的原始空白（原文末尾的换行等）。
    tail: String,
}

/// 把长文本切分成不超过 `limit` 字符的块，并保留块间的原始空白。
///
/// 三级策略：
/// 1. 先按空行（段落）切，段落不超过 limit 就独立成块
/// 2. 段落超长，再按句末标点切
/// 3. 还超长，只能硬切（这种情况很少见，通常是没标点的长串）
///
/// 不变量：`Σ(piece.before + piece.text) + tail` 恰好等于原文。
/// 译文按同样的空白拼回去，结构与缩进不会因为切块而改变。
fn split_text(text: &str, limit: usize) -> Split {
    // 中英文的句末标点都认
    const SENTENCE_ENDS: [char; 6] = ['。', '！', '？', '.', '!', '?'];

    // 上限为 0 会退化成无限循环；后端不会给出 0，这里只做防御
    let limit = limit.max(1);
    let mut pieces: Vec<Piece> = Vec::new();
    // 尚未归属到任何块的空白（文档开头、块与块之间）
    let mut pending = String::new();
    // 当前块的前置空白，块开始时从 pending 锁定
    let mut before = String::new();
    let mut buf = String::new();

    // 结束当前块；块尾若带空白（硬切恰好切在空白后），交还给下一块。
    fn flush(buf: &mut String, before: &mut String, pieces: &mut Vec<Piece>) -> String {
        let trailing = buf.split_off(buf.trim_end().len());
        pieces.push(Piece {
            before: std::mem::take(before),
            text: std::mem::take(buf),
        });
        trailing
    }

    for ch in text.chars() {
        if ch.is_whitespace() {
            if buf.is_empty() {
                pending.push(ch);
                continue;
            }
            buf.push(ch);
            // 空白也可能是第 limit 个字符：块不能超限
            if char_len(&buf) >= limit {
                pending = flush(&mut buf, &mut before, &mut pieces);
            }
            continue;
        }

        // 块以空白结尾时，先判断这段空白是不是段落边界。
        // 单个空白属于同一段，必须留在块内（英文句间的空格不能被吃掉）。
        if buf.chars().last().is_some_and(char::is_whitespace) {
            let whitespace = buf.split_off(buf.trim_end().len());
            if whitespace.matches('\n').count() >= 2 {
                // 段落边界：结束当前块，空行原样交给下一块
                pieces.push(Piece {
                    before: std::mem::take(&mut before),
                    text: std::mem::take(&mut buf),
                });
                pending = whitespace;
            } else {
                buf.push_str(&whitespace);
            }
        }

        // 从 pending 锁定本块的前导空白
        if buf.is_empty() {
            before = std::mem::take(&mut pending);
        }
        buf.push(ch);

        // 句子边界（攒够 limit/2 才切，避免每句发一次请求）或硬上限
        let len = char_len(&buf);
        if (SENTENCE_ENDS.contains(&ch) && len >= limit / 2) || len >= limit {
            pending = flush(&mut buf, &mut before, &mut pieces);
        }
    }

    let mut tail = String::new();
    if !buf.is_empty() {
        tail = flush(&mut buf, &mut before, &mut pieces);
    }
    // buf 已空时，pending 就是原文末尾的空白
    tail.push_str(&pending);

    Split { pieces, tail }
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
    use std::future::Future;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    // ── 切块 ──

    /// 按「前置空白 + 内容」原样拼回，验证切分可逆
    fn 拼回(split: &Split) -> String {
        let mut out = String::new();
        for piece in &split.pieces {
            out.push_str(&piece.before);
            out.push_str(&piece.text);
        }
        out.push_str(&split.tail);
        out
    }

    #[test]
    fn 短文本不切分() {
        let split = split_text("你好世界", 100);
        assert_eq!(split.pieces.len(), 1);
        assert_eq!(split.pieces[0].before, "");
        assert_eq!(split.pieces[0].text, "你好世界");
        assert_eq!(split.tail, "");
    }

    #[test]
    fn 按段落切分且空行不丢() {
        let split = split_text("第一段。\n\n第二段。\n\n第三段。", 10);
        assert_eq!(split.pieces.len(), 3);
        assert_eq!(split.pieces[1].before, "\n\n");
        assert_eq!(split.pieces[2].before, "\n\n");
    }

    #[test]
    fn 超长段落按句子切() {
        let long = "这是第一句话。这是第二句话。这是第三句话。这是第四句话。";
        let split = split_text(long, 12);
        // 应该被切成多块，且每块不超过上限
        assert!(split.pieces.len() > 1);
        for piece in &split.pieces {
            assert!(char_len(&piece.text) <= 12, "块超长了：{}", piece.text);
        }
    }

    #[test]
    fn 不带标点的长串也能切() {
        let s = "a".repeat(50);
        let split = split_text(&s, 10);
        assert!(split.pieces.len() >= 5);
        for piece in &split.pieces {
            assert!(char_len(&piece.text) <= 10);
        }
    }

    /// 切分必须可逆：空行、块首缩进、文档首尾空白都要留在原位。
    ///
    /// 这是修掉「join("\n") 把空行和缩进吃掉」的守门测试 —— 旧的
    /// 「只检查非空白字符还在」根本发现不了结构被改写。
    #[test]
    fn 切分可逆_空行缩进与首尾空白都保留() {
        let text = "  第一段。\n\n第二段。\n\n\n  缩进行\n尾行\n";
        let split = split_text(text, 100);

        assert_eq!(拼回(&split), text);
        assert_eq!(split.pieces[0].before, "  ");
        assert_eq!(split.pieces[2].before, "\n\n\n  ");
        assert_eq!(split.tail, "\n");
    }

    /// 英文句子之间是空格，不是换行 —— 拼接时不能凭空插入 `\n`
    #[test]
    fn 英文句间空格保留为空格() {
        let split = split_text("One. Two. Three.", 6);
        assert!(split.pieces.len() > 1);
        assert_eq!(split.pieces[1].before, " ");
        assert_eq!(拼回(&split), "One. Two. Three.");
    }

    // ── 回退链（脚本化后端）──

    /// 脚本化后端：记录收到的文本，可选择失败或原样返回。
    ///
    /// 回退、按后端分键、二次切分都是纯机制，不该靠真实网络来验证；
    /// 有了它，`translate_one` 的每条分支都能确定性地跑到。
    struct Stub {
        name: &'static str,
        max_chars: usize,
        fail: bool,
        echo: bool,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl Stub {
        fn new(name: &'static str, max_chars: usize) -> Self {
            Self {
                name,
                max_chars,
                fail: false,
                echo: false,
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn failing(mut self) -> Self {
            self.fail = true;
            self
        }

        /// 原样返回输入，用于验证译文拼接保真
        fn echoing(mut self) -> Self {
            self.echo = true;
            self
        }

        fn sharing(mut self, calls: Arc<Mutex<Vec<String>>>) -> Self {
            self.calls = calls;
            self
        }
    }

    impl Backend for Stub {
        fn name(&self) -> &str {
            self.name
        }

        fn max_chars(&self) -> usize {
            self.max_chars
        }

        fn translate<'a>(
            &'a self,
            req: Request,
        ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
            let name = self.name;
            let fail = self.fail;
            let echo = self.echo;
            let calls = Arc::clone(&self.calls);
            Box::pin(async move {
                calls.lock().unwrap().push(req.text.clone());
                if fail {
                    return Err(PoryError::Backend(format!("{name} 故障")));
                }
                if echo {
                    Ok(req.text)
                } else {
                    Ok(format!("⟦{name}⟧{}", req.text))
                }
            })
        }
    }

    fn 任务(text: &str) -> TranslateJob {
        TranslateJob {
            text: text.to_string(),
            from: Lang::parse("en").unwrap(),
            to: Lang::parse("zh").unwrap(),
            to_if_same: None,
        }
    }

    async fn 跑(backends: Vec<Box<dyn Backend>>, text: &str) -> Report {
        Translator::new(backends).run(&任务(text)).await
    }

    /// 建一个独立的临时缓存路径，绝不碰真实的用户缓存目录
    fn 临时缓存(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pory-translator-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("cache.json")
    }

    #[tokio::test]
    async fn 主后端失败时回退并显示真实路径() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let secondary = Stub::new("secondary", 100).sharing(Arc::clone(&log));
        let report = 跑(
            vec![
                Box::new(Stub::new("primary", 100).failing()),
                Box::new(secondary),
            ],
            "hello",
        )
        .await;

        assert_eq!(report.outcome.unwrap(), "⟦secondary⟧hello");
        assert_eq!(report.stats.backends, ["primary", "secondary"]);
        assert!(report
            .stats
            .warnings
            .iter()
            .any(|w| w.contains("primary 失败，回退到 secondary")));
        assert_eq!(*log.lock().unwrap(), ["hello"]);
        assert_eq!(report.stats.misses, 1);
        assert_eq!(report.stats.hits, 0);
    }

    /// 切块粒度跟主后端走：AI 的 8000 不会被兜底后端的 400 拖累
    #[tokio::test]
    async fn 主后端上限决定切块粒度() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let backend = Stub::new("primary", 4).sharing(Arc::clone(&log));
        let report = 跑(vec![Box::new(backend)], "abcdefghij").await;

        assert_eq!(*log.lock().unwrap(), ["abcd", "efgh", "ij"]);
        assert_eq!(report.stats.misses, 3);
    }

    /// 轮到上限更小的后端时，在那一层再切一次
    #[tokio::test]
    async fn 回退时按该后端上限二次切分() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let secondary = Stub::new("secondary", 4).sharing(Arc::clone(&log));
        let report = 跑(
            vec![
                Box::new(Stub::new("primary", 100).failing()),
                Box::new(secondary),
            ],
            "abcdefgh",
        )
        .await;

        assert_eq!(*log.lock().unwrap(), ["abcd", "efgh"]);
        assert_eq!(report.outcome.unwrap(), "⟦secondary⟧abcd⟦secondary⟧efgh");
        assert_eq!(report.stats.misses, 2);
    }

    #[tokio::test]
    async fn 全部后端失败时保留最后错误与回退警告() {
        let report = 跑(
            vec![
                Box::new(Stub::new("a", 10).failing()),
                Box::new(Stub::new("b", 10).failing()),
            ],
            "hi",
        )
        .await;

        assert!(report.outcome.unwrap_err().to_string().contains("b 故障"));
        assert_eq!(report.stats.backends, ["a", "b"]);
        assert_eq!(report.stats.warnings.len(), 1, "只有 a→b 那次回退要报");
    }

    /// 回退产出的译文只能挂在**实际后端**的键下。
    /// 否则 AI 恢复后会命中 MyMemory 的低质量结果，而且看不出来。
    #[tokio::test]
    async fn 回退结果只缓存在实际后端键下() {
        let path = 临时缓存("fallback-cache");

        // 第一次：主后端失败，结果来自 secondary 并落盘
        let mut translator = Translator::new(vec![
            Box::new(Stub::new("primary", 100).failing()),
            Box::new(Stub::new("secondary", 100)),
        ])
        .with_cache(Cache::at(&path));
        assert_eq!(
            translator.run(&任务("hello")).await.outcome.unwrap(),
            "⟦secondary⟧hello"
        );

        // 第二次：主后端仍坏，应命中 secondary 的缓存、不再发请求
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut translator = Translator::new(vec![
            Box::new(Stub::new("primary", 100).failing()),
            Box::new(Stub::new("secondary", 100).sharing(Arc::clone(&log))),
        ])
        .with_cache(Cache::at(&path));
        let report = translator.run(&任务("hello")).await;
        assert_eq!(report.stats.hits, 1);
        assert!(log.lock().unwrap().is_empty(), "命中缓存不应发请求");

        // 第三次：主后端恢复。不能命中 secondary 留下的低质量译文
        let mut translator =
            Translator::new(vec![Box::new(Stub::new("primary", 100))]).with_cache(Cache::at(&path));
        let report = translator.run(&任务("hello")).await;
        assert_eq!(report.outcome.unwrap(), "⟦primary⟧hello");
        assert_eq!(report.stats.hits, 0, "不能串到 secondary 的键");

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn refresh_跳过读缓存但仍写回() {
        let path = 临时缓存("refresh");

        let mut translator =
            Translator::new(vec![Box::new(Stub::new("primary", 100))]).with_cache(Cache::at(&path));
        assert_eq!(translator.run(&任务("hello")).await.stats.misses, 1);

        let mut translator = Translator::new(vec![Box::new(Stub::new("primary", 100))])
            .with_cache(Cache::at(&path))
            .with_refresh(true);
        let report = translator.run(&任务("hello")).await;
        assert_eq!(report.stats.hits, 0, "--refresh 不读缓存");
        assert_eq!(report.stats.misses, 1);

        // 刷新后的结果已写回：第三次普通运行应当命中
        let mut translator =
            Translator::new(vec![Box::new(Stub::new("primary", 100))]).with_cache(Cache::at(&path));
        assert_eq!(translator.run(&任务("hello")).await.stats.hits, 1);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 端到端：切块 → 翻译 → 拼回，结构必须一字不差
    #[tokio::test]
    async fn 译文拼接保留原文结构与缩进() {
        let text = "第一段。\n\n第二段。\n\n\n  缩进行\n尾行";
        let report = 跑(vec![Box::new(Stub::new("echo", 100).echoing())], text).await;
        assert_eq!(report.outcome.unwrap(), text);
    }
}
