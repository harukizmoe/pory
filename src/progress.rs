//! 等待动画 —— 联网翻译要花时间，这段时间不该是一片空白。
//!
//! 形态参考大模型的 thinking 动画：一个**旋转的字形** + 一行**走动的秒数**，
//! 文案上有一道微光淌过。
//!
//! 只在 **stderr 是终端**时启用。管道、CI、重定向里一个字节都不多写 ——
//! 加这个功能之前的行为完全不变（项目铁律：新功能必须 opt-in，
//! 而这条铁律的立意就是「可脚本化、可预测」，守住了）。
//!
//! 四个「不做」，每一个都是为了不撒谎或不添乱：
//! - **不做进度条**。我们不知道还要等多久，任何百分比都是编的。
//! - **不显示块数**。等待时人只关心「在动」，不关心内部切了几块。
//! - **不隐藏光标**（不用 `\x1b[?25l`）。被 Ctrl-C 打断时恢复序列不会执行，
//!   光标就永久消失，得敲 `reset` 才救得回来。代价只是光标停在行尾。
//! - **不留残影**。结束时擦掉整行，不跳行、不多打一个空行。

use owo_colors::OwoColorize;
use std::io::IsTerminal;
use std::time::Duration;

/// 每帧间隔。80 ms ≈ 12.5 fps：快到看得出在转，慢到不刷屏。
pub const FRAME: Duration = Duration::from_millis(80);

/// 第一帧的延迟。
///
/// 缓存命中只要 4 ms —— 那时画一帧再擦掉，比不画更糟（闪一下像故障）。
/// 让第一帧等到 150 ms，绝大多数缓存命中根本不会看到动画。
/// 实现上不需要任何额外判断：计时器的第一拍本来就落在这个时刻之后。
pub const FIRST_FRAME: Duration = Duration::from_millis(150);

/// 盲文点旋转帧。
///
/// 挑它是因为**实测过**：这十个字形在 Maple Mono NF CN 里自带
/// （advance=600 = 1 格），不会走 fallback，也就不会错位。
/// （Claude 那种 `✻`/`✳` 在这套字体里没有，别用。）
const SPIN: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// 等待文案。用中文：运行时提示一直如此（英文只出现在 `--help` 里）。
const LABEL: &str = "正在翻译";

/// 动画该不该出现。
///
/// **唯一的判据是 stderr 是不是终端。** 不是终端就一帧都不画，
/// 于是一个字节都不会进入管道 / 日志 / CI 输出。
pub fn enabled() -> bool {
    std::io::stderr().is_terminal()
}

/// 这一帧光带的波峰落在文案的第几个字上（`-1` 和 `len` 表示波峰还在文案外 ——
/// 那是光带进出画面的两拍，此时没有字被点亮）。
///
/// 单独抽出来是为了能直接测「光带真的扫过了每一个字」，
/// 而不必去解析 ANSI 转义码。
fn peak(frame: usize) -> isize {
    let span = LABEL.chars().count() + 2;
    (frame / 2 % span) as isize - 1
}

/// 渲染一帧的完整文本。
///
/// 微光带每**两帧**推进一格（160 ms），比旋转字形慢一半 ——
/// 字形在「转」、光在「淌」，两个频率不同才不会显得呆板。
/// 波峰从 -1 走到 len，让光带完整地飘出去再回来，
/// 而不是在边缘凭空出现、凭空消失。
pub fn frame_text(frame: usize, elapsed: Duration) -> String {
    let cells: Vec<char> = LABEL.chars().collect();
    let head = peak(frame);

    // 光带用「亮 → 中 → 暗」三档：#0 是波峰，±1 是余晖，
    // 这样看着像一道光晕，而不是一个硬边矩形在爬。
    // 底色一律暗灰 —— 「有颜色」的位置永远在动，静止的地方没有颜色。
    let mut body = String::new();
    for (i, ch) in cells.iter().enumerate() {
        let lit = ch.to_string();
        body.push_str(&match (i as isize - head).abs() {
            0 => lit.bright_cyan().to_string(),
            1 => lit.cyan().to_string(),
            _ => lit.bright_black().to_string(),
        });
    }

    format!(
        "{} {} {}",
        SPIN[frame % SPIN.len()].to_string().bright_cyan(),
        body,
        format!("{:.1}s", elapsed.as_secs_f32()).bright_black(),
    )
}

/// 画一帧：回到行首、清行、写整行。
///
/// 为什么用 `\x1b[2K` 清行，而不是补空格对齐？因为补空格要先算**可见宽度**，
/// 而 `正在翻译` 是 CJK（每字占 2 格）、秒数的位数还会从 1 位涨到 2 位 ——
/// 算错一个格就是一行残影。清行不需要知道任何宽度。
///
/// 也不需要手动 flush：Rust 的 stderr 是**无缓冲**的，`eprint!` 立刻落盘。
/// 这正是 `\r` 就地重绘能成立的前提。
pub fn draw(frame: usize, elapsed: Duration) {
    eprint!("\r\x1b[2K{}", frame_text(frame, elapsed));
}

/// 擦掉动画行，把光标留在行首 —— 让后面紧接着的输出从干净的一行开始。
pub fn clear() {
    eprint!("\r\x1b[2K");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 去掉 ANSI 转义序列，只留可见文本
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut in_esc = false;
        for ch in s.chars() {
            if in_esc {
                if ch.is_ascii_alphabetic() {
                    in_esc = false;
                }
            } else if ch == '\x1b' {
                in_esc = true;
            } else {
                out.push(ch);
            }
        }
        out
    }

    /// 每一帧的可见宽度必须完全一样，否则 `\r` 重绘会留下残影
    #[test]
    fn 每帧可见宽度一致() {
        let span = (LABEL.chars().count() + 2) * 2 + SPIN.len();
        let widths: Vec<usize> = (0..span)
            .map(|f| {
                strip_ansi(&frame_text(f, Duration::from_millis(1234)))
                    .chars()
                    .count()
            })
            .collect();
        assert!(
            widths.iter().all(|w| *w == widths[0]),
            "各帧宽度应当一致，实际：{widths:?}"
        );
    }

    /// 字形逐帧更换 —— 这是「在动」的全部依据
    #[test]
    fn 旋转字形逐帧变化() {
        let first: String = (0..SPIN.len())
            .map(|f| strip_ansi(&frame_text(f, Duration::ZERO)))
            .map(|s| s.chars().next().unwrap())
            .collect();
        assert_eq!(first, "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");
    }

    /// 秒数是**实测耗时**（`Instant` 数出来的），不是估算 ——
    /// 同一帧传不同的耗时必须跟着变，这是「不撒谎」的落点
    #[test]
    fn 秒数来自实测耗时() {
        let a = strip_ansi(&frame_text(0, Duration::from_millis(1234)));
        let b = strip_ansi(&frame_text(0, Duration::from_millis(9876)));
        assert!(a.starts_with("⠋ 正在翻译"), "实际：{a}");
        assert!(a.ends_with("1.2s"), "实际：{a}");
        assert!(b.ends_with("9.9s"), "实际：{b}");
    }

    /// 光带必须真的扫过文案的每一个字，而不是钉在某处
    #[test]
    fn 微光带扫过每一个字() {
        let span = LABEL.chars().count() + 2;
        let mut seen = vec![false; LABEL.chars().count()];
        for f in 0..span * 2 {
            let p = peak(f);
            if p >= 0 && (p as usize) < seen.len() {
                seen[p as usize] = true;
            }
        }
        assert!(seen.iter().all(|s| *s), "有字没被光扫到：{seen:?}");
    }

    /// 一个周期内波峰只能前进，不能来回弹 —— 来回弹看着犹豫
    #[test]
    fn 光带单向推进() {
        let span = LABEL.chars().count() + 2;
        let seq: Vec<isize> = (0..span).map(|f| peak(f * 2)).collect();
        assert!(
            seq.windows(2).all(|w| w[1] > w[0]),
            "波峰应当单调前进：{seq:?}"
        );
    }
}
