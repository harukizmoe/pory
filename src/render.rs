//! 终端展示层：把一次翻译的结果与统计渲染到 stdout / stderr。
//!
//! 这一层只回答「长什么样」，不参与翻译、不碰网络、不改统计。
//! 与 `main` 分离是因为展示规则（stdout/stderr 分工、颜色只在终端时才加、
//! 左侧符号的对齐栅格）本身就是一套独立规范，理由都在下面的注释里。

use crate::translator::{Stats, TranslateJob};
use owo_colors::OwoColorize;
use std::io::IsTerminal;

/// 打印翻译结果。
///
/// 终端里的样子（2026-09-20 定的形态）：
///
/// ```text
/// ↳ 原文（只有输入来自管道 / 文件时才回显，最暗）
/// 译文（stdout，加粗、不上色）
/// ◈ zh-CN › en · mymemory · 18 blocks · cache 12/18（stderr，次要）
/// ```
///
/// 四条纪律：
/// 1. **译文走 stdout，且只在 stdout 是终端时才加粗**。`pory "..." > out.txt`
///    必须得到干净的纯文本 —— 一个转义码都不能有。这跟 `bat`、`ls --color=auto`
///    是同一套做法：有没有人在看，决定要不要打扮。
/// 2. **元信息走 stderr、放在译文下方**。长文本滚两屏也顶不掉它，
///    它始终贴在屏底。
/// 3. **亮度只有两级**（译文亮、元信息与原文暗），颜色只在异常时出现 ——
///    「有颜色」本身就是一条信息，不能拿来当装饰。
///    译文也因此**不上色，只加粗**：提示符本来就是彩的，译文再用强调色就会
///    和命令行撞成一团（见下面译文的说明），而且 pory 猜不到用户的提示符配色。
/// 4. **左侧符号全部落在第 0 列，内容从第 2 列开始**。`◈` / `↳` / `⚠` 和动画的
///    `⠋` 都是 1 格宽（字体里实测过），所以 pory 的每一条 stderr 行都和
///    命令行文字**同列**（常见提示符是「1 格符号 + 1 空格」= 2 格）。
///    这不是装饰，是让「pory 说的」和「用户敲的」在同一条栅格上。
pub fn print_result(
    job: &TranslateJob,
    result: &str,
    plain: bool,
    stats: &Stats,
    refresh: bool,
    use_cache: bool,
    echo_source: bool,
) {
    if plain {
        // 极简模式：只要译文。连脚注都不要 —— 它的用途就是喂给别的程序。
        println!("{result}");
        return;
    }

    // 原文回显。判据是「原文在不在眼前」，而不是「信息多不多」：
    // 终端手打时它还挂在提示符上，只有管道 / 文件输入时才需要重复一遍。
    // 放在译文**上方**，因为它读在前、译在后，正是双语对的自然顺序。
    if echo_source {
        let line = format!("↳ {}", truncate(&job.text, 120));
        // 颜色只在 stderr 是终端时加（铁律：颜色只给看得见的流）——
        // `pory 2> log` 的日志文件里不能有转义码。
        if std::io::stderr().is_terminal() {
            eprintln!("{}", line.bright_black());
        } else {
            eprintln!("{line}");
        }
    }

    // 译文：强调，但**只加粗、不上色**。
    //
    // 原来是 `bold + cyan`，问题在提示符上：用户的提示符是彩色渐变
    // （粉→紫→蓝→青），`$Pory 你好呀` 本身就是青的，译文再青一遍，
    // 命令行和结果就糊成一团 —— 一眼看不出哪行是敲的、哪行是翻出来的。
    //
    // 改成素色加粗后对比反而更强：**提示符是彩的，译文是素的**。
    // 而且这不去猜用户的提示符配色 —— 换任何主题都不会再撞。
    // 颜色仍按纪律留给状态（回退的琥珀、警告），译文一个字都不占。
    if std::io::stdout().is_terminal() {
        println!("{}", result.bold());
    } else {
        println!("{result}");
    }

    // 脚注：次要信息。发生过回退（链里留下了不止一个后端名）时整行转琥珀 ——
    // 异常占的是同一个位置，只是换了颜色，所以不必靠多堆一行来让人看见。
    // 同原文回显：颜色只在 stderr 是终端时加 —— 这条是 2026-09-21 补的，
    // 之前漏了检测，`pory 2> log` 会把 ANSI 转义码写进日志（性能测评脚本
    // 解析脚注耗时的时候当场抓到）。
    let note = footnote(job, stats, refresh, use_cache);
    if std::io::stderr().is_terminal() {
        if stats.backends.len() > 1 {
            eprintln!("{}", note.yellow());
        } else {
            eprintln!("{}", note.bright_black());
        }
    } else {
        eprintln!("{note}");
    }
}

/// 脚注行首的标记。
///
/// 挑 `◈`（U+25C8，菱形里嵌着菱形）是因为它一个字符就把两件事说清了：
/// - **跟名字的关联** —— pory 取自多边兽 Porygon，`poly` 就是「多边」；
///   这个字形本身就是「由多边形构成的多边形」，正是多边兽那副
///   多面体拼起来的样子。不是一个随便找来的装饰符。
/// - **跟候选池比它为什么胜出** —— 实测过一圈多边形类字符
///   （`△▽◁▷◇◆◊◉◎◕⊕✶` 都在字体里，`⬢⬡⎔⌬` 这些立体感更强的**没有**），
///   `◈` 是唯一在 Maple Mono NF CN / Adwaita Mono / DejaVu Sans Mono
///   三款等宽字体里都有、且 advance 正好 600（**恰好 1 格**）的那一个。
///   宽度必须是 1 格：它后面紧跟一个空格，两格正好等于常见提示符
///   `❯ ` 的宽度，脚注文字才能和命令行文字**同列**（见 print_result 纪律 4）。
const MARK: &str = "◈";

/// 组装译文下方那行元信息，形如
/// `◈ zh-CN › en · mymemory · 18 blocks · cache 12/18 · 1.2s`。
///
/// 每一段都有真实含义，没有一段是凑数的：
/// - **语向**：`⇄` 表示「方向由检测结果定」（互翻开启时二者之一），
///   `›` 表示确定方向。
/// - **后端**：出现多个名字（如 `ai → mymemory`）就说明发生过回退 ——
///   静默回退是不诚实的，必须让人看见。
/// - **块数**：`18 blocks` 是真实的请求块数（含缓存命中；回退到上限更小的
///   后端时会二次切分），所以这个数字不撒谎。
/// - **缓存**：`cache 12/18` 是「命中 12 块 / 共 18 块」。
/// - **耗时**（2026-09-21 应用户要求加入）：整次翻译的墙钟时间，一位小数秒。
///   缓存命中时显示 `0.0s` —— 如实报告「瞬时」，与动画「命中不闪帧」同一哲学。
fn footnote(job: &TranslateJob, stats: &Stats, refresh: bool, use_cache: bool) -> String {
    let blocks = stats.hits + stats.misses;
    let mut parts = vec![lang_pair(job), stats.backends.join(" → ")];

    if blocks > 0 {
        parts.push(format!(
            "{blocks} block{}",
            if blocks == 1 { "" } else { "s" }
        ));
    }

    // 缓存文案按状态分档。关闭和「强制刷新」是两种明确的状态，
    // 用 `cache 18/18` 这种数字表达不了，所以单独给词。
    if !use_cache {
        parts.push("cache off".to_string());
    } else if refresh {
        parts.push("cache refreshed".to_string());
    } else if blocks > 0 {
        parts.push(format!("cache {}/{}", stats.hits, blocks));
    }

    // 耗时放最末尾 —— 它是对「刚才那一等值不值」的直接回答。
    // 一位小数足够分辨「命中（0.0s）」与「真请求（几秒）」，再多一位是噪音。
    parts.push(format!("{:.1}s", stats.duration.as_secs_f64()));

    format!("{MARK} {}", parts.join(" · "))
}

/// 语向的显示。
///
/// 互翻开启时**实际方向是运行时按检测结果定的**（原文是第一语言就翻成第二语言，
/// 反之亦然），所以这里只能写 `⇄`（二者之一）。写 `→` 就是声称一个当时并不
/// 成立的方向 —— 旧的头部 `auto → zh-CN / en` 正是因为这个被换掉的。
fn lang_pair(job: &TranslateJob) -> String {
    match &job.to_if_same {
        Some(alt) => format!("{} ⇄ {}", job.to.code(), alt.code()),
        None => format!("{} › {}", job.from.code(), job.to.code()),
    }
}

/// 把长文本压成一行并截断，用于预览显示
fn truncate(s: &str, max: usize) -> String {
    let one_line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        one_line
    } else {
        let cut: String = one_line.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Lang;

    /// 造一个翻译任务，用于测脚注
    fn 任务(from: &str, to: &str, 备用: Option<&str>) -> TranslateJob {
        TranslateJob {
            text: "hello".to_string(),
            from: Lang::parse(from).unwrap(),
            to: Lang::parse(to).unwrap(),
            to_if_same: 备用.map(|l| Lang::parse(l).unwrap()),
        }
    }

    /// 造一份统计，用于测脚注
    fn 统计(hits: usize, misses: usize) -> Stats {
        Stats {
            hits,
            misses,
            backends: vec!["mymemory".to_string()],
            warnings: Vec::new(),
            duration: std::time::Duration::ZERO,
        }
    }

    /// 脚注的形态就是规格里那行：`◈ zh › en · mymemory · 18 blocks · cache 12/18 · 0.0s`
    ///
    /// 行首必须是 `MARK + 一个空格` —— 这两格是对齐的基准：
    /// 它让脚注文字落下时和命令行文字同列（见 print_result 纪律 4）。
    #[test]
    fn 脚注按规格拼装() {
        let note = footnote(&任务("zh", "en", None), &统计(12, 6), false, true);
        assert_eq!(
            note,
            "◈ zh-CN › en · mymemory · 18 blocks · cache 12/18 · 0.0s"
        );
        assert!(
            note.starts_with(&format!("{MARK} ")),
            "行首的标记与空格是对齐基准，不能少：{note}"
        );
    }

    /// 耗时在脚注最末尾，一位小数秒（1340ms → 1.3s）
    #[test]
    fn 脚注尾部带耗时() {
        let mut stats = 统计(0, 1);
        stats.duration = std::time::Duration::from_millis(1340);
        let note = footnote(&任务("zh", "en", None), &stats, false, true);
        assert!(note.ends_with(" · 1.3s"), "实际：{note}");
    }

    /// 只有一块时用单数 —— `1 blocks` 是硬伤，不能忍
    #[test]
    fn 单块用单数() {
        let note = footnote(&任务("zh", "en", None), &统计(0, 1), false, true);
        assert!(note.contains(" · 1 block · "), "实际：{note}");
        assert!(!note.contains("1 blocks"), "实际：{note}");
    }

    /// 关缓存、强制刷新各自给词，而不是端出 `cache 0/18` 这种说不清的账
    /// （耗时是脚注最后一段，断言用 contains 而非 ends_with）
    #[test]
    fn 缓存关闭与刷新各有文案() {
        let off = footnote(&任务("zh", "en", None), &统计(0, 18), false, false);
        assert!(off.contains("cache off"), "实际：{off}");

        let refreshed = footnote(&任务("zh", "en", None), &统计(0, 18), true, true);
        assert!(refreshed.contains("cache refreshed"), "实际：{refreshed}");
    }

    /// 互翻时实际方向由检测结果定，只能用 `⇄`；写 `→` 就是说了句不成立的话
    #[test]
    fn 互翻时用双向箭头() {
        let note = footnote(&任务("auto", "zh", Some("en")), &统计(18, 0), false, true);
        assert!(note.contains("zh-CN ⇄ en"), "实际：{note}");
        assert!(!note.contains('›'), "互翻时不该出现单向箭头：{note}");
    }

    /// 没指定源语言时如实写 `auto`，不要假装知道方向
    #[test]
    fn 未检测出源语言时如实写_auto() {
        let note = footnote(&任务("auto", "en", None), &统计(0, 1), false, true);
        assert!(note.contains("auto › en"), "实际：{note}");
    }

    /// 回退过（链里不止一个后端名）必须是看得见的
    #[test]
    fn 回退路径进脚注() {
        let mut stats = 统计(0, 1);
        stats.backends = vec!["ai".to_string(), "mymemory".to_string()];
        let note = footnote(&任务("zh", "en", None), &stats, false, true);
        assert!(note.contains("ai → mymemory"), "实际：{note}");
        assert!(
            stats.backends.len() > 1,
            "这正是 print_result 判 Amber 的依据"
        );
    }
}
