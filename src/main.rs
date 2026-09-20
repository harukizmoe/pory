//! pory —— 终端里的语言转换器。
//!
//! 用法示例：
//! ```text
//! pory "hello world"              # 翻成默认目标语言
//! pory "hello" -t ja              # 翻成日语
//! pory "こんにちは" -t zh -f ja    # 明确指定源和目标
//! cat README.md | pory -t en      # 管道输入
//! echo "test" | pory -b google    # 换后端
//! pory --init                     # 生成配置文件
//! pory -b ai "专业术语" -t en      # 用 AI 后端
//! pory "已经是中文的内容"           # auto：检测出就是第一语言 → 改译第二语言
//! ```

// 每个源文件对应一个模块，声明在这里
mod backend;
mod cache;
mod config;
mod error;
mod lang;
mod progress;
mod translator;

use clap::Parser;
use std::io::Read;

use backend::ai::Ai;
use backend::google::Google;
use backend::mymemory::MyMemory;
use backend::Backend;
use cache::Cache;
use config::Config;
use error::{PoryError, Result};
use lang::Lang;
use std::io::IsTerminal;
use translator::{Stats, TranslateJob, Translator};

/// 命令行参数定义。
///
/// clap 的 derive 宏会读这些注解自动生成解析器、--help 和错误提示。
/// `#[arg(...)]` 里的 short/long 决定短选项和长选项的名字。
///
/// ⚠️ 下面每个字段的 doc comment **不是给读代码的人看的注释** ——
/// 它就是**给用户看的 --help 文本**，所以一律写**英文**。
/// （2026-09-20 决定：CLI 的对外描述只保留一种语言，即英文；不做中英双语。）
/// 这是全项目唯一允许出现英文的位置，别"顺手"把它们改回中文 ——
/// 改它们等于改用户界面。真正的内部注释仍然全部中文。
#[derive(Parser, Debug)]
#[command(
    name = "pory",
    version,
    about = "A language converter in your terminal",
    long_about = "pory turns one set of symbols into another.\n\n\
                  Reads from stdin when no text is given, so it pipes well:\n  \
                  echo hello | pory -t ja"
)]
struct Cli {
    /// Text to translate. If omitted, read from stdin (pipes supported)
    text: Option<String>,

    /// Target language, e.g. zh / en / ja
    #[arg(short = 't', long)]
    target: Option<String>,

    /// Source language, defaults to auto detection
    #[arg(short = 'f', long)]
    from: Option<String>,

    /// Translation backend: mymemory / google / ai
    ///
    /// On its own with no text in a terminal, it is saved to the config file as
    /// the new default backend instead of translating:
    ///
    ///   pory -b google    -> writes backend = "google"
    ///
    /// With text, or with piped input, it applies to this call only:
    ///
    ///   pory -b google "text"
    #[arg(short = 'b', long)]
    backend: Option<String>,

    /// Print the translation only: no color, no footnote, no progress animation
    #[arg(long)]
    plain: bool,

    /// Don't use the cache: neither read existing entries nor write new ones
    #[arg(long)]
    no_cache: bool,

    /// Ignore the existing cache, re-translate and update it
    #[arg(long)]
    refresh: bool,

    /// Clear the translation cache and exit
    #[arg(long)]
    clear_cache: bool,

    /// Write a sample config file and exit
    #[arg(long)]
    init: bool,
}

// 用**单线程**异步运行时，而不是默认的多线程。
//
// 两个理由：
// 1. pory 的请求是串行的（见 translator.rs 的说明），多线程没有收益
// 2. 更要紧的是**输出顺序**：多线程下 async 函数可能在 worker 线程
//    执行，与主线程的 println!/eprintln! 并发写同一个终端，
//    导致输出交错（实测出现过 `⚠ a⚠ ai 失败...` 这种撕裂）。
//    单线程运行时可彻底避免这个问题，且省掉线程调度开销、启动更快。
#[tokio::main(flavor = "current_thread")]
async fn main() {
    // 解析命令行。参数不合法时 clap 会自己打印提示并退出。
    let cli = Cli::parse();

    // 处理 --init：生成配置模板后直接结束
    if cli.init {
        match Config::save() {
            Ok(path) => {
                println!("✓ 已生成配置文件：{}", path.display());
                println!("  打开它填入 API Key（如果用 AI 后端）或调整默认语种。");
            }
            Err(e) => {
                eprintln!("✗ {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // 处理 --clear-cache：清空缓存后直接结束
    if cli.clear_cache {
        match Cache::clear() {
            Ok(Some(path)) => println!("✓ 已清空缓存：{}", path.display()),
            Ok(None) => println!("缓存本来就是空的，无需清理。"),
            Err(e) => {
                eprintln!("✗ {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // 处理「设置默认后端」：`pory -b <名字>` 不带文本、且 stdin 是终端时，
    // 它是持久设置而非翻译（详见 try_set_backend 的说明）。
    match try_set_backend(&cli) {
        Ok(Some((name, path))) => {
            println!("✓ 已把默认后端设为 {name}（{}）", path.display());
            println!("  以后不带 -b 的翻译都会用它。");
            // 顺手确认这个后端**现在**能不能用。否则用户敲完就以为设好了，
            // 下次翻译撞上「主后端不可用」的警告才发现缺 Key —— 困惑点离得很远。
            // 只提醒、不阻止：缺的那个配置（如 API Key）可能马上就补上。
            if let Err(e) = build_backend(&name, &Config::load()) {
                eprintln!("⚠ 注意：{name} 现在还不能用 —— {e}");
                eprintln!("  翻译时会自动改用保底后端，不会中断。");
            }
            return;
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    }

    // 把 async 逻辑包一层，出错时统一打印并返回非零退出码
    if let Err(e) = run(cli).await {
        eprintln!("✗ {e}");
        // 退出码 1 让 shell 能感知失败（脚本里可判断）
        std::process::exit(1);
    }
}

/// 真正的执行流程
async fn run(cli: Cli) -> Result<()> {
    // ── 1. 载入配置 ──
    // 配置提供默认值，命令行参数优先级更高（覆盖配置）
    let cfg = Config::load();

    // ── 2. 确定输入文本 ──
    // 先记下「输入是不是从 stdin 来的」—— 下面要移走 cli.text，之后就问不出来了。
    // 这个布尔只用于一件事：决定要不要回显原文（见 print_result）。
    let echo_source = cli.text.is_none();
    let text = resolve_input(cli.text)?;

    // ── 3. 确定语种 ──
    // 优先级：命令行 > 配置文件默认值
    let from = Lang::parse(cli.from.as_deref().unwrap_or(&cfg.source))?;
    // 配置里的 `primary` 就是「不指定 -t 时翻成什么」
    let to = match &cli.target {
        Some(t) => Lang::parse(t)?,
        None => Lang::parse(&cfg.primary)?,
    };
    // 第一语言 / 第二语言互翻（只在 auto 模式且未显式指定 -t 时生效）
    let to_if_same = resolve_swap(&cfg, &from, &to, cli.target.is_some());

    // ── 4. 构建后端链（主后端 + 保底备选）──
    let primary_name = cli.backend.as_deref().unwrap_or(&cfg.backend);
    let backends = build_chain(&cfg, primary_name)?;

    // ── 5. 执行翻译 ──
    let job = TranslateJob {
        text,
        from,
        to,
        to_if_same,
    };

    let mut translator = Translator::new(backends);

    // 缓存开关。两个来源，效果相同（完全不碰缓存：既不读也不写）：
    //   --no-cache      —— 只管这一次
    //   cache = false   —— 配置文件里的持久开关，管以后每次都这样
    // 命令行优先：加了 --no-cache 就用不上缓存，配置说 true 也没意义。
    let use_cache = !cli.no_cache && cfg.cache;
    if use_cache {
        translator = translator.with_cache(Cache::load());
    }
    // --refresh 时忽略已有缓存，强制重新翻译
    if cli.refresh {
        translator = translator.with_refresh(true);
    }

    let result = if !cli.plain && progress::enabled() {
        // 终端里带等待动画。管道 / CI / 重定向里 stderr 不是终端，
        // 这条分支根本不会走 —— 那些场景的行为与加动画之前完全一致。
        translate_with_progress(&mut translator, &job).await?
    } else {
        translator.run(&job).await?
    };

    // ── 6. 输出 ──
    let stats = translator.stats();

    // 翻译过程中攒下的警告到这里才打印。为什么不在发生处直接打？
    // 因为那会儿 stderr 的当前行被动画占着，直接打印会接在动画行尾巴上，
    // 而下一帧的 `\x1b[2K` 会把它连同一起擦掉 —— 那等于静默。详见 Stats::warnings。
    for w in &stats.warnings {
        eprintln!("⚠ {w}");
    }

    print_result(
        &job,
        &result,
        cli.plain,
        &stats,
        cli.refresh,
        use_cache,
        echo_source,
    );

    Ok(())
}

/// 带等待动画地跑一次翻译。
///
/// 为什么用 `select!` 把「请求」和「计时器」放在**同一条线程**里轮询，
/// 而不是另起一个线程画动画？
/// 因为本项目在「两个写者」上栽过：当初 tokio 多线程运行时让 async 写 stderr
/// 与主线程输出交错，出现过 `⚠ a⚠ ai 失败` 这种撕裂。`select!` 全程在同一条
/// 线程里，**结构上不可能撕裂** —— 同一时刻只有一处能写 stderr。
///
/// 请求一回来就立刻擦掉动画行，紧接着打印的结果落在干净的位置上：
/// 不跳行、不留残影、也不多一个空行。
async fn translate_with_progress(
    translator: &mut Translator,
    job: &TranslateJob,
) -> Result<String> {
    // 把请求钉住（pin），这样 tick 触发时**不会把它丢掉** ——
    // 每轮 select 重新轮询的是同一个 future，请求只发一次。
    let fut = translator.run(job);
    tokio::pin!(fut);

    // `interval_at` 的第一拍落在 FIRST_FRAME 之后，于是
    // 「请求比 150 ms 快 → 一帧都不画」不需要任何额外判断。
    let mut ticker = tokio::time::interval_at(
        tokio::time::Instant::now() + progress::FIRST_FRAME,
        progress::FRAME,
    );
    let start = std::time::Instant::now();
    let mut frame = 0usize;
    let mut drawn = false;

    loop {
        tokio::select! {
            out = &mut fut => {
                if drawn {
                    progress::clear();
                }
                return out;
            }
            _ = ticker.tick() => {
                progress::draw(frame, start.elapsed());
                frame += 1;
                drawn = true;
            }
        }
    }
}

/// 「设置默认后端」形态的判定与执行。
///
/// `pory -b <名字>` 单独出现、且 stdin 是终端时，含义是**持久设置**：
/// 写进配置文件，以后每次都用它。这样「指定后端」就和缓存一样有了两种粒度 ——
/// 带文本是「这一次」，不带文本是「以后每次都这样」。
///
/// 判定刻意严格，三个条件缺一不可：
/// 1. **没有文本参数，且 stdin 是终端**。`cat x | pory -b google` 同样没有文本参数，
///    但那是要翻译，所以必须排除管道 —— 这是「有没有输入」不能只看参数的原因。
/// 2. **给了 `-b`**。
/// 3. **`-b` 是唯一的参数**。若同时出现 `-t` / `--plain` 之类，说明本意是翻译
///    （只是忘了带文本）—— 那就不猜意图去改配置，交给用法提示更安全。
///
/// 名字不认识时直接报错、不写进配置 —— 写进去等于以后每次启动都报错。
fn try_set_backend(cli: &Cli) -> Result<Option<(String, std::path::PathBuf)>> {
    if cli.text.is_some() || !std::io::stdin().is_terminal() {
        return Ok(None);
    }
    let Some(name) = cli.backend.as_deref() else {
        return Ok(None);
    };
    if cli.target.is_some() || cli.from.is_some() || cli.plain || cli.no_cache || cli.refresh {
        return Ok(None);
    }

    if !backend::is_known(name) {
        return Err(PoryError::Config(format!(
            "未知后端 `{name}`。可用：{}",
            backend::KNOWN.join(" / ")
        )));
    }

    let path = Config::set_backend(name)?;
    Ok(Some((name.to_string(), path)))
}

/// 构建后端链：主后端在前，保底备选按配置顺序跟在后面。
///
/// **主后端构建失败不直接报错**，而是警告一声后继续建保底链。
///
/// 这条规则是 2026-09-20 改的。原先主后端构建失败用 `?` 直接返回，于是
/// `backend = "ai"` 但没填 Key 时，保底链那个循环**根本没机会跑** ——
/// 配好的 `fallback = ["mymemory"]` 形同虚设，整个工具不能用。
///
/// 原注释的理由是「静默降级会让用户以为在用 AI，实际在用机翻，那是欺骗」——
/// 防的方向没错，但手段过头了：保底链**本来就不是静默的**（有警告、头部
/// 也显示实际用的后端）。把「可见」等同于「报错退出」，代价是工具直接不可用。
/// 正确做法是让失败**更可见**（警告 + 显示真实后端），而不是让工具罢工。
///
/// 只有**一个后端都建不出来**时才报错，且优先报主后端的原因 ——
/// 那是用户明确指定的那个，解释力最强（如「未知后端 foo」直接指出写错了配置）。
fn build_chain(cfg: &Config, primary_name: &str) -> Result<Vec<Box<dyn Backend>>> {
    let mut chain: Vec<Box<dyn Backend>> = Vec::new();
    let mut primary_err: Option<PoryError> = None;
    // 构建失败**先记下来不打印** —— 万一整条链都是空的，主后端的错误
    // 会作为最终错误返回给用户，那时再打一遍警告就是同一句话说了两遍。
    let mut skipped: Vec<(String, String)> = Vec::new();

    match build_backend(primary_name, cfg) {
        Ok(b) => chain.push(b),
        Err(e) => primary_err = Some(e),
    }

    // 回退链：按配置顺序尝试，跳过与已有后端重名的
    for name in &cfg.fallback {
        if chain.iter().any(|b| b.name() == name) {
            continue;
        }
        // 回退后端构建失败（比如 AI 没配 Key）不该中断本次翻译，
        // 跳过它继续用后面的即可。
        match build_backend(name, cfg) {
            Ok(b) => chain.push(b),
            Err(e) => skipped.push((name.clone(), e.to_string())),
        }
    }

    if chain.is_empty() {
        return Err(primary_err.unwrap_or_else(|| {
            PoryError::Config(format!(
                "没有可用的翻译后端。可用：{}",
                backend::KNOWN.join(" / ")
            ))
        }));
    }

    // 到这儿说明有后端能用，此时「谁没用上」才是值得报告的信息
    // （上面全空的分支已经直接报错了，不会走到这里重复一遍）。
    if let Some(e) = primary_err {
        eprintln!("⚠ 主后端 `{primary_name}` 不可用，将改用保底后端：{e}");
    }
    for (name, why) in skipped {
        eprintln!("⚠ 跳过保底后端 `{name}`：{why}");
    }

    Ok(chain)
}

/// 按名字构造一个后端。
///
/// 抽成独立函数是因为要构造多次（主后端 + 若干回退后端）。
fn build_backend(name: &str, cfg: &Config) -> Result<Box<dyn Backend>> {
    match name {
        "mymemory" => Ok(Box::new(MyMemory::new(cfg.mymemory_email.clone()))),
        "google" => Ok(Box::new(Google::new(cfg.google_endpoint.clone()))),
        "ai" => {
            // AI 后端必须有 key，缺了给个明确指引而不是让请求失败
            if cfg.ai.api_key.trim().is_empty() {
                return Err(PoryError::Config(
                    "AI 后端需要 API Key。请运行 `pory --init` 生成配置，\
                     然后在 [ai] 段填入 api_key；\
                     若想先用免 Key 的后端，把配置里的 backend 改成 mymemory。"
                        .into(),
                ));
            }
            Ok(Box::new(Ai::new(
                cfg.ai.base_url.clone(),
                cfg.ai.api_key.clone(),
                cfg.ai.model.clone(),
            )))
        }
        other => Err(PoryError::Config(format!(
            "未知后端 `{other}`。可用：{}",
            backend::KNOWN.join(" / ")
        ))),
    }
}

/// 计算「第一语言 / 第二语言互翻」的备用目标。
///
/// 三个前置条件，缺一不换方向：
/// 1. **源语言是 auto** —— 只有待检测时才知道原文是不是第一语言；
///    显式给了 `-f` 说明用户已经告诉我们源语言了。
/// 2. **用户没给 `-t`** —— 显式指定目标就是明确指令，不该被自动换掉。
/// 3. **配置了 `secondary`** —— 留空即关闭互翻。
///
/// 配置里写了无法识别的语种时**只警告并关闭互翻**，不中断翻译 ——
/// 与 `fallback` 里遇到不认识的后端名同一种处理。
fn resolve_swap(cfg: &Config, from: &Lang, to: &Lang, target_given: bool) -> Option<Lang> {
    if !from.is_auto() || target_given || cfg.secondary.trim().is_empty() {
        return None;
    }

    match Lang::parse(&cfg.secondary) {
        // 两个语言一样的话互翻没有意义（会翻成自己）
        Ok(alt) if alt != *to => Some(alt),
        Ok(_) => None,
        Err(e) => {
            eprintln!("⚠ 配置里的 secondary 无效，已关闭互翻：{e}");
            None
        }
    }
}

/// 连续输入都在读 stdin 时用的错误提示（避免重复写三遍）
const USAGE_HINT: &str = "没有可翻译的内容。用法：\n  \
     pory \"要翻译的文本\"\n  \
     echo \"文本\" | pory -t ja\n\
     查看全部用法：pory --help";

fn resolve_input(cli_text: Option<String>) -> Result<String> {
    // 情况一：命令行直接给了文本
    if let Some(t) = cli_text {
        if t.trim().is_empty() {
            return Err(PoryError::Input(USAGE_HINT.into()));
        }
        return Ok(t);
    }

    // 情况二：没给文本，尝试读 stdin。
    //
    // 但 **stdin 是终端时直接给用法提示，不去读** —— 终端上永远等不到输入，
    // `read_to_string` 会一直阻塞到 EOF（用户得按 Ctrl-D 才能脱身），
    // 观感就是「敲完命令卡住」。管道和重定向不受影响（它们的 stdin 不是终端）。
    //
    // （2026-09-20 补：这段判断原来没有，当时的注释声称「终端没输入」与
    // 「空管道」反应相同，所以不用判终端 —— 实测是错的：空管道立即可读、
    // 立即报提示，终端则阻塞。注释写的是它以为的行为。）
    if std::io::stdin().is_terminal() {
        return Err(PoryError::Input(USAGE_HINT.into()));
    }

    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_ok() && !buf.trim().is_empty() {
        return Ok(buf);
    }

    Err(PoryError::Input(USAGE_HINT.into()))
}

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
fn print_result(
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

    use owo_colors::OwoColorize;

    // 原文回显。判据是「原文在不在眼前」，而不是「信息多不多」：
    // 终端手打时它还挂在提示符上，只有管道 / 文件输入时才需要重复一遍。
    // 放在译文**上方**，因为它读在前、译在后，正是双语对的自然顺序。
    if echo_source {
        eprintln!(
            "{}",
            format!("↳ {}", truncate(&job.text, 120)).bright_black()
        );
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
    let note = footnote(job, stats, refresh, use_cache);
    if stats.backends.len() > 1 {
        eprintln!("{}", note.yellow());
    } else {
        eprintln!("{}", note.bright_black());
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
/// `◈ zh-CN › en · mymemory · 18 blocks · cache 12/18`。
///
/// 每一段都有真实含义，没有一段是凑数的：
/// - **语向**：`⇄` 表示「方向由检测结果定」（互翻开启时二者之一），
///   `›` 表示确定方向。
/// - **后端**：出现多个名字（如 `ai → mymemory`）就说明发生过回退 ——
///   静默回退是不诚实的，必须让人看见。
/// - **块数**：`18 blocks` 是真实的切块数（400 字符一块，切分在本地完成，
///   不依赖网络），所以这个数字不撒谎。
/// - **缓存**：`cache 12/18` 是「命中 12 块 / 共 18 块」。
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

    /// 造一份「第二语言是指定值」的配置
    fn 配置(second: &str) -> Config {
        Config {
            secondary: second.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn auto_且未指定目标时启用互翻() {
        let cfg = 配置("en");
        let from = Lang::parse("auto").unwrap();
        let to = Lang::parse(&cfg.primary).unwrap();
        assert_eq!(
            resolve_swap(&cfg, &from, &to, false),
            Some(Lang::parse("en").unwrap())
        );
    }

    #[test]
    fn 显式指定目标时不换方向() {
        // 用户写了 -t zh，那是明确指令，不该被自动改成 en
        let cfg = 配置("en");
        let from = Lang::parse("auto").unwrap();
        let to = Lang::parse("zh").unwrap();
        assert_eq!(resolve_swap(&cfg, &from, &to, true), None);
    }

    #[test]
    fn 显式指定源语言时不换方向() {
        // 源语言已知，不需要靠检测判断「原文是不是第一语言」
        let cfg = 配置("en");
        let from = Lang::parse("ja").unwrap();
        let to = Lang::parse("zh").unwrap();
        assert_eq!(resolve_swap(&cfg, &from, &to, false), None);
    }

    #[test]
    fn second_留空则关闭互翻() {
        let cfg = 配置("   ");
        let from = Lang::parse("auto").unwrap();
        let to = Lang::parse("zh").unwrap();
        assert_eq!(resolve_swap(&cfg, &from, &to, false), None);
    }

    #[test]
    fn second_与第一语言相同时关闭互翻() {
        // 互翻成自己等于没翻，还可能与第一语言来回绕
        let cfg = 配置("zh-CN");
        let from = Lang::parse("auto").unwrap();
        let to = Lang::parse("zh").unwrap();
        assert_eq!(resolve_swap(&cfg, &from, &to, false), None);
    }

    #[test]
    fn second_不合法时只关闭互翻不中断() {
        // 与 fallback 里遇到不认识的后端名同一种处理：警告 + 跳过
        let cfg = 配置("klingon");
        let from = Lang::parse("auto").unwrap();
        let to = Lang::parse("zh").unwrap();
        assert_eq!(resolve_swap(&cfg, &from, &to, false), None);
    }

    /// 主后端不可用（AI 缺 Key）时**不能让保底链失效**。
    ///
    /// 这是「`pory -b ai` 之后整个工具不能用」那个 bug 的守门测试：
    /// 当时主后端构建失败用 `?` 直接返回，保底链的循环根本没跑。
    #[test]
    fn 主后端缺_key_时仍用保底后端() {
        let cfg = Config {
            backend: "ai".to_string(),
            ..Default::default()
        };
        assert!(cfg.ai.api_key.is_empty(), "本测试的前提是没配 Key");

        let chain = build_chain(&cfg, "ai").expect("保底链可用，不该报错");
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].name(), "mymemory");
    }

    /// 主后端名字不认识时同理：有保底就继续，而不是整个不能用
    #[test]
    fn 主后端名字未知时仍用保底后端() {
        let cfg = Config::default();
        let chain = build_chain(&cfg, "no-such-backend").expect("保底链可用，不该报错");
        assert_eq!(chain[0].name(), "mymemory");
    }

    /// 一个后端都建不出来才报错，且报的是主后端的原因（最有解释力）
    #[test]
    fn 全部不可用时才报错并给出主后端原因() {
        let cfg = Config {
            backend: "ai".to_string(),
            fallback: Vec::new(),
            ..Default::default()
        };
        // 用 match 而非 unwrap_err：Box<dyn Backend> 没有 Debug，unwrap_err 编译不过
        let msg = match build_chain(&cfg, "ai") {
            Ok(_) => panic!("一个后端都建不出来，应当报错"),
            Err(e) => e.to_string(),
        };
        assert!(msg.contains("API Key"), "应报主后端的原因，实际：{msg}");
    }

    /// 正常情况：链首是主后端，且与保底重名时去重
    #[test]
    fn 正常时链首为主后端且自动去重() {
        let cfg = Config::default(); // backend 与 fallback 都是 mymemory
        let chain = build_chain(&cfg, "mymemory").unwrap();
        assert_eq!(chain.len(), 1, "同一个后端不该在链里出现两次");
        assert_eq!(chain[0].name(), "mymemory");
    }

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
        }
    }

    /// 脚注的形态就是规格里那行：`◈ zh › en · mymemory · 18 blocks · cache 12/18`
    ///
    /// 行首必须是 `MARK + 一个空格` —— 这两格是对齐的基准：
    /// 它让脚注文字落下时和命令行文字同列（见 print_result 纪律 4）。
    #[test]
    fn 脚注按规格拼装() {
        let note = footnote(&任务("zh", "en", None), &统计(12, 6), false, true);
        assert_eq!(note, "◈ zh-CN › en · mymemory · 18 blocks · cache 12/18");
        assert!(
            note.starts_with(&format!("{MARK} ")),
            "行首的标记与空格是对齐基准，不能少：{note}"
        );
    }

    /// 只有一块时用单数 —— `1 blocks` 是硬伤，不能忍
    #[test]
    fn 单块用单数() {
        let note = footnote(&任务("zh", "en", None), &统计(0, 1), false, true);
        assert!(note.contains(" · 1 block · "), "实际：{note}");
        assert!(!note.contains("1 blocks"), "实际：{note}");
    }

    /// 关缓存、强制刷新各自给词，而不是端出 `cache 0/18` 这种说不清的账
    #[test]
    fn 缓存关闭与刷新各有文案() {
        let off = footnote(&任务("zh", "en", None), &统计(0, 18), false, false);
        assert!(off.ends_with("cache off"), "实际：{off}");

        let refreshed = footnote(&任务("zh", "en", None), &统计(0, 18), true, true);
        assert!(refreshed.ends_with("cache refreshed"), "实际：{refreshed}");
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
        assert!(stats.backends.len() > 1, "这正是 print_result 判 Amber 的依据");
    }
}
