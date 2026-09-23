//! pory —— 终端里的语言转换器。
//!
//! 用法示例：
//! ```text
//! pory "hello world"              # 翻成默认目标语言
//! pory "hello" -t ja              # 翻成日语
//! pory "こんにちは" -t zh -f ja    # 明确指定源和目标
//! cat README.md | pory -t en      # 管道输入
//! echo "test" | pory -b traditional   # 这一次只用免 Key 机翻
//! pory --init                     # 生成配置文件
//! pory "已经是中文的内容"           # auto：检测出就是第一语言 → 改译第二语言
//! ```

// 每个源文件对应一个模块，声明在这里
mod backend;
mod cache;
mod config;
mod dictionary;
mod error;
mod lang;
mod progress;
mod shell;
mod translator;
mod uninstall;

use clap::{Parser, Subcommand};
use std::future::Future;
use std::io::Read;

use backend::Backend;
use cache::Cache;
use config::{Config, Mode, TRADITIONAL_KNOWN};
use error::{PoryError, Result};
use lang::Lang;
use std::io::IsTerminal;
use translator::{Stats, TranslateJob, Translator};

#[derive(Subcommand, Debug, Clone)]
enum Command {
    /// Look up a word or phrase; quote multiword terms
    Dict { term: String },
}

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
#[derive(Parser, Debug, Clone)]
#[command(
    name = "pory",
    version,
    about = "A terminal translator and dictionary",
    long_about = "pory translates text and looks up dictionary entries.\n\n\
                  Reads from stdin when no text is given, so it pipes well:\n  \
                  echo hello | pory -t ja\n\
                  pory dict hello"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Text to translate. If omitted, read from stdin (pipes supported)
    text: Option<String>,

    /// Target language, e.g. zh / en / ja
    #[arg(short = 't', long, global = true)]
    target: Option<String>,

    /// Source language, defaults to auto detection
    #[arg(short = 'f', long, global = true)]
    from: Option<String>,

    /// Translation mode: ai / traditional
    ///
    /// On its own with no text in a terminal, it is saved to the config file
    /// as the default mode instead of translating:
    ///
    ///   pory -b traditional    -> writes mode = "traditional"
    ///
    /// With text, or with piped input, it applies to this call only:
    ///
    ///   pory -b traditional "text"
    #[arg(short = 'b', long)]
    backend: Option<String>,

    /// Print the translation only: no color, no footnote, no progress animation
    #[arg(long)]
    plain: bool,

    /// Don't read or write translation or dictionary cache entries
    #[arg(long, global = true)]
    no_cache: bool,

    /// Ignore cached results and regenerate the translation or entry
    #[arg(long, global = true)]
    refresh: bool,

    /// Maximum wall-clock time for one command (default: 300 seconds; range: 1-86400)
    #[arg(long, value_name = "SECONDS", value_parser = parse_timeout_secs, global = true)]
    timeout: Option<u64>,

    /// Clear the translation and dictionary cache, then exit
    #[arg(long)]
    clear_cache: bool,

    /// Write a sample config file and exit
    ///
    /// Refuses to overwrite an existing config file (never clobbers your settings).
    #[arg(long)]
    init: bool,

    /// Install shell integration (short name + completions) and exit
    ///
    ///   fish   writes ~/.config/fish/completions/pory.fish (fish loads it automatically)
    ///
    ///   bash   writes a completion file and appends `alias pr=pory` to ~/.bashrc
    ///
    ///   zsh    prints the completion script and appends the alias to ~/.zshrc
    ///          (zsh completions are untested; fish and bash are verified)
    ///
    /// Adds are idempotent and marked, so they can be undone (see --uninstall).
    #[arg(long, value_name = "SHELL", value_parser = ["fish", "bash", "zsh"])]
    init_shell: Option<String>,

    /// Print the shell integration to stdout instead of installing it (only with --init-shell)
    ///
    /// Handy to review it, change the short name, or place it yourself:
    ///
    ///   pory --init-shell fish --print
    #[arg(long, requires = "init_shell")]
    print: bool,

    /// Remove everything pory added to this system and exit
    ///
    /// Cleans shell integration (completion files + rc aliases) and the
    /// translation and dictionary cache without asking. The config file is kept unless you
    /// confirm its deletion at the prompt.
    #[arg(long)]
    uninstall: bool,
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

    // 处理 --init-shell：默认**直接安装**到该 shell 的标准补全位置（这是用户主动运行的
    // 安装命令，没有理由让他自己去拼路径 + 重定向）；--print 则只输出脚本。
    if let Some(name) = cli.init_shell.as_deref() {
        match shell::run(name, cli.print) {
            Ok(out) => {
                // 脚本自带结尾换行（走 print!），安装提示没有（走 println!）
                if out.ends_with('\n') {
                    print!("{out}");
                } else {
                    println!("{out}");
                }
            }
            Err(e) => {
                eprintln!("✗ {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // 处理 --uninstall：清理 shell 集成与缓存（默认、不询问），
    // 配置文件问一次（y 才删）。它自己边做边打印 —— 交互提问必须紧跟上下文。
    if cli.uninstall {
        if let Err(e) = uninstall::run() {
            eprintln!("✗ {e}");
            std::process::exit(1);
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

    // 处理「设置默认模式」：`pory -b <模式>` 不带文本、且 stdin 是终端时，
    // 它是持久设置而非翻译（详见 try_set_mode 的说明）。
    match try_set_mode(&cli) {
        Ok(Some((mode, path))) => {
            println!("✓ 已把默认翻译模式设为 {mode}（{}）", path.display());
            println!("  以后不带 -b 的翻译都会按它路由。");
            // 顺手确认这个模式**现在**能不能真正用上 AI。否则用户敲完就以为
            // 切到 AI 了，下次翻译才发现一直在用传统机翻 —— 困惑点离得很远。
            // 只提醒、不阻止：Key 可能马上就补上。
            if mode == Mode::Ai {
                let cfg = Config::load();
                let has_ready = cfg.ai.order.iter().any(|n| {
                    cfg.ai
                        .providers
                        .get(n)
                        .map(|p| !p.api_key.trim().is_empty() && !p.models.is_empty())
                        .unwrap_or(false)
                });
                if !has_ready {
                    eprintln!("⚠ 还没有任何可用的 AI 提供商（需填 api_key 且列出 models），翻译会直接用传统机翻。");
                    eprintln!("  打开配置文件，按 [ai] 段注释填一个免费档（如智谱）即可用上 AI。");
                }
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
async fn run(mut cli: Cli) -> Result<()> {
    let cfg = Config::load();

    if let Some(Command::Dict { term }) = cli.command.as_ref() {
        return run_dictionary(&cli, &cfg, term).await;
    }

    let echo_source = cli.text.is_none();
    let text = resolve_input(cli.text.take())?;
    run_translation(&cli, &cfg, text, echo_source, None).await
}

/// 普通翻译与词典失败降级共用同一条执行路径。
async fn run_translation(
    cli: &Cli,
    cfg: &Config,
    text: String,
    echo_source: bool,
    timeout_override: Option<std::time::Duration>,
) -> Result<()> {
    let from = Lang::parse(cli.from.as_deref().unwrap_or(&cfg.source))?;
    let to = match &cli.target {
        Some(target) => Lang::parse(target)?,
        None => Lang::parse(&cfg.primary)?,
    };
    let to_if_same = resolve_swap(cfg, &from, &to, cli.target.is_some());
    let mode = match cli.backend.as_deref() {
        Some(name) => parse_mode(name)?,
        None => cfg.mode,
    };
    let backends = build_mode_chain(cfg, mode)?;
    let job = TranslateJob {
        text,
        from,
        to,
        to_if_same,
    };

    let mut translator = Translator::new(backends);
    let use_cache = !cli.no_cache && cfg.cache;
    if use_cache {
        translator = translator.with_cache(Cache::load());
    }
    if cli.refresh {
        translator = translator.with_refresh(true);
    }
    if let Some(timeout) =
        timeout_override.or_else(|| cli.timeout.map(std::time::Duration::from_secs))
    {
        translator = translator.with_timeout(timeout);
    }

    let result = if !cli.plain && progress::enabled() {
        translate_with_progress(&mut translator, &job).await
    } else {
        translator.run(&job).await
    };
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            for warning in &translator.stats().warnings {
                eprintln!("⚠ {warning}");
            }
            return Err(error);
        }
    };

    let stats = translator.stats();
    for warning in &stats.warnings {
        eprintln!("⚠ {warning}");
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

async fn run_dictionary(cli: &Cli, cfg: &Config, term: &str) -> Result<()> {
    if cli.backend.is_some() {
        return Err(PoryError::Input("`-b` 不适用于词典查词".into()));
    }
    if cli.plain {
        return Err(PoryError::Input("`--plain` 不适用于词典查词".into()));
    }
    let term = term.trim();
    if term.is_empty() {
        return Err(PoryError::Input("待查词条不能为空".into()));
    }

    let from = Lang::parse(cli.from.as_deref().unwrap_or(&cfg.source))?;
    let to = match &cli.target {
        Some(target) => Lang::parse(target)?,
        None => Lang::parse(&cfg.primary)?,
    };
    let to_if_same = resolve_swap(cfg, &from, &to, cli.target.is_some());
    if (!from.is_auto() && !dictionary::supports_language(&from))
        || !dictionary::supports_language(&to)
        || to_if_same
            .as_ref()
            .is_some_and(|lang| !dictionary::supports_language(lang))
    {
        return Err(PoryError::Input(
            "词典目前支持中文、日语和英语；请用 -f / -t 指定语种".into(),
        ));
    }

    let timeout = std::time::Duration::from_secs(
        cli.timeout
            .unwrap_or(translator::DEFAULT_TRANSLATION_TIMEOUT.as_secs()),
    );
    let started = std::time::Instant::now();
    let candidates = backend::ai::configured(cfg);
    let use_cache = !cli.no_cache && cfg.cache;
    let mut cache = use_cache.then(Cache::load);
    let lookup = dictionary::lookup(dictionary::LookupRequest {
        term,
        from: &from,
        to: &to,
        to_if_same: to_if_same.as_ref(),
        models: &candidates.models,
        cache: cache.as_mut(),
        refresh: cli.refresh,
        timeout,
    });
    let result = if progress::enabled() {
        with_progress("正在查词", lookup).await
    } else {
        lookup.await
    };

    match result {
        Ok(result) => {
            report_ai_candidates(&candidates);
            if let Some(cache) = cache.as_ref() {
                if let Err(error) = cache.save() {
                    eprintln!("⚠ 词条缓存保存失败（不影响本次结果）：{error}");
                }
            }
            println!(
                "{}",
                dictionary::render(&result.entry, std::io::stdout().is_terminal())
            );
            let cache_status = if !use_cache {
                "cache off"
            } else if result.cached {
                "cache hit"
            } else {
                "AI generated"
            };
            let status = format!(
                "◈ AI 生成（非权威） · {} · {cache_status} · {:.1}s",
                result.model,
                started.elapsed().as_secs_f64()
            );
            if std::io::stderr().is_terminal() {
                use owo_colors::OwoColorize;
                eprintln!("{}", status.bright_black());
            } else {
                eprintln!("{status}");
            }
            Ok(())
        }
        Err(error) => {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining < std::time::Duration::from_secs(1) {
                return Err(PoryError::DictionaryTimeout(timeout.as_secs()));
            }
            eprintln!("⚠ AI 词条不可用，降级为普通翻译（仅翻译，非词条）：{error}");
            run_translation(cli, cfg, term.to_string(), false, Some(remaining)).await
        }
    }
}

/// 在单线程运行时等待任意请求，并显示指定文案。
///
/// 请求和动画在同一线程轮询，避免 stderr 同时写入导致输出交错。
async fn with_progress<F: Future>(label: &str, future: F) -> F::Output {
    let fut = future;
    tokio::pin!(fut);
    let mut ticker = tokio::time::interval_at(
        tokio::time::Instant::now() + progress::FIRST_FRAME,
        progress::FRAME,
    );
    let start = std::time::Instant::now();
    let mut frame = 0usize;
    let mut drawn = false;

    loop {
        tokio::select! {
            output = &mut fut => {
                if drawn {
                    progress::clear();
                }
                return output;
            }
            _ = ticker.tick() => {
                progress::draw_for(label, frame, start.elapsed());
                frame += 1;
                drawn = true;
            }
        }
    }
}

async fn translate_with_progress(
    translator: &mut Translator,
    job: &TranslateJob,
) -> Result<String> {
    with_progress(progress::TRANSLATION_LABEL, translator.run(job)).await
}

/// 「设置默认模式」形态的判定与执行。
///
/// `pory -b <模式>` 单独出现、且 stdin 是终端时，含义是**持久设置**：
/// 写进配置文件的 `mode`，以后每次都按它路由。这样「指定模式」就和缓存一样
/// 有了两种粒度 —— 带文本是「这一次」，不带文本是「以后每次都这样」。
///
/// 判定刻意严格，三个条件缺一不可：
/// 1. **没有文本参数，且 stdin 是终端**。`cat x | pory -b ai` 同样没有文本参数，
///    但那是要翻译，所以必须排除管道 —— 这是「有没有输入」不能只看参数的原因。
/// 2. **给了 `-b`**。
/// 3. **`-b` 是唯一的参数**。若同时出现 `-t` / `--plain` 之类，说明本意是翻译
///    （只是忘了带文本）—— 那就不猜意图去改配置，交给用法提示更安全。
///
/// 名字不是合法模式时直接报错、不写进配置 —— 写进去等于以后每次启动都报错。
fn try_set_mode(cli: &Cli) -> Result<Option<(Mode, std::path::PathBuf)>> {
    if cli.command.is_some() || cli.text.is_some() || !std::io::stdin().is_terminal() {
        return Ok(None);
    }
    let Some(name) = cli.backend.as_deref() else {
        return Ok(None);
    };
    if cli.target.is_some()
        || cli.from.is_some()
        || cli.plain
        || cli.no_cache
        || cli.refresh
        || cli.timeout.is_some()
    {
        return Ok(None);
    }

    let mode = parse_mode(name)?;
    let path = Config::set_mode(mode)?;
    Ok(Some((mode, path)))
}

/// 把 `-b` 的值解析成模式。模式的合法值只有两个 ——
/// 这里是它们唯一的定义点，报错文案与校验都以它为准。
fn parse_mode(name: &str) -> Result<Mode> {
    match name {
        "ai" => Ok(Mode::Ai),
        "traditional" => Ok(Mode::Traditional),
        other => Err(PoryError::Config(format!(
            "未知翻译模式 `{other}`。可用：ai / traditional"
        ))),
    }
}

/// Parse the total translation timeout. Bound it to one day to avoid instant overflow.
fn parse_timeout_secs(value: &str) -> std::result::Result<u64, String> {
    let seconds = value
        .parse::<u64>()
        .map_err(|_| "timeout must be a positive integer".to_string())?;
    if (1..=86_400).contains(&seconds) {
        Ok(seconds)
    } else {
        Err("timeout must be between 1 and 86400 seconds".to_string())
    }
}

fn report_ai_candidates(candidates: &backend::ai::AiCandidates) {
    if !candidates.no_key.is_empty() {
        eprintln!(
            "⚠ AI 提供商未填 api_key，已跳过：{}",
            candidates.no_key.join("、")
        );
    }
    for (name, why) in &candidates.skipped {
        eprintln!("⚠ 跳过后端 `{name}`：{why}");
    }
}

/// 按翻译模式构建后端链。
///
/// **mode 路由只发生在链构建**：产出仍是一条平链，调度与回退机制
/// （translator.rs）对「模式」毫不知情 —— 这是双模式设计的支点，
/// 也兑现了「策略在上层、机制在后端」的分工。
///
/// - **Ai 模式**：`ai.order` 里的提供商逐个构建（没填 api_key 的跳过），
///   然后**拼接** traditional 全链 —— AI 全挂时传统兜底，工具永远可用。
/// - **Traditional 模式**：只有 traditional 链，完全不碰 AI（省 tokens）。
///
/// 这条设计是 2026-09-21 定稿的，但它延续 2026-09-20 的教训：
/// 主后端不可用时**继续建链而不是让工具罢工** —— 当年 `backend="ai"` 缺 Key
/// 时保底链根本没跑，工具直接不可用。现在的形态从结构上杜绝了这个类别的 bug：
/// AI 提供商缺 Key 根本不会进链，传统链永远在后面候着。
///
/// 警告打印的时机：构建发生在动画开始之前，stderr 当前行还没被占用，
/// 直接 eprintln 是安全的（动画期打印会被 `\x1b[2K` 擦掉，见 Stats::warnings）。
///
/// 警告的合并规则：全部 AI 提供商都缺 api_key 时合并成**一条**警告
/// —— 原因相同，逐条报等于同一句话刷屏；其他跳过原因各不相同，逐条报。
fn build_mode_chain(cfg: &Config, mode: Mode) -> Result<Vec<Box<dyn Backend>>> {
    let mut chain: Vec<Box<dyn Backend>> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut ai_ready = 0usize;

    match mode {
        Mode::Ai => {
            let candidates = backend::ai::configured(cfg);
            ai_ready = candidates.models.len();
            report_ai_candidates(&candidates);
            chain.extend(
                candidates
                    .models
                    .into_iter()
                    .map(|ai| Box::new(ai) as Box<dyn Backend>),
            );

            // AI 与传统后端使用不同名字空间；传统链始终保留作兜底。
            for name in &cfg.traditional.order {
                if chain.iter().any(|backend| backend.name() == name) {
                    continue;
                }
                match backend::build_traditional(name, cfg) {
                    Ok(backend) => chain.push(backend),
                    Err(error) => skipped.push((name.clone(), error.to_string())),
                }
            }
        }
        Mode::Traditional => {
            for name in &cfg.traditional.order {
                if chain.iter().any(|backend| backend.name() == name) {
                    continue;
                }
                match backend::build_traditional(name, cfg) {
                    Ok(backend) => chain.push(backend),
                    Err(error) => skipped.push((name.clone(), error.to_string())),
                }
            }
        }
    }

    for (name, why) in &skipped {
        eprintln!("⚠ 跳过后端 `{name}`：{why}");
    }

    if chain.is_empty() {
        return Err(PoryError::Config(match mode {
            Mode::Ai => "没有可用的翻译后端（AI 提供商与传统后端都不可用）".into(),
            Mode::Traditional => format!(
                "传统后端列表没有可用项。可用：{}",
                TRADITIONAL_KNOWN.join(" / ")
            ),
        }));
    }

    if mode == Mode::Ai && ai_ready == 0 {
        eprintln!("⚠ 没有可用的 AI 提供商，本次将直接使用传统机翻。");
        eprintln!("  打开配置文件，按 [ai] 段注释填一个提供商（含 api_key）即可用上 AI 翻译。");
    }

    Ok(chain)
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
/// - **块数**：`18 blocks` 是真实的切块数（400 字符一块，切分在本地完成，
///   不依赖网络），所以这个数字不撒谎。
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
    use config::ProviderConfig;

    #[test]
    fn dict_短语必须作为单个参数传入() {
        let cli = Cli::try_parse_from(["pory", "dict", "break the ice"]).unwrap();
        match cli.command {
            Some(Command::Dict { term }) => assert_eq!(term, "break the ice"),
            _ => panic!("应解析为词典子命令"),
        }
        assert!(Cli::try_parse_from(["pory", "dict", "break", "the", "ice"]).is_err());
    }

    #[tokio::test]
    async fn dict_拒绝翻译专用选项() {
        let cfg = Config::default();
        for args in [
            vec!["pory", "--plain", "dict", "hello"],
            vec!["pory", "-b", "traditional", "dict", "hello"],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            let term = match cli.command.as_ref() {
                Some(Command::Dict { term }) => term,
                _ => panic!("应解析为词典子命令"),
            };
            assert!(run_dictionary(&cli, &cfg, term)
                .await
                .unwrap_err()
                .to_string()
                .contains("不适用于词典查词"));
        }
        assert!(Cli::try_parse_from(["pory", "dict", "hello", "--plain"]).is_err());
        assert!(Cli::try_parse_from(["pory", "dict", "hello", "-b", "traditional"]).is_err());
    }

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

    // ── build_mode_chain 的测试 ──

    /// 造一份配置：AI order 里列出若干提供商名
    fn 配置_ai(order: &[&str]) -> Config {
        let mut cfg = Config::default();
        cfg.ai.order = order.iter().map(|s| s.to_string()).collect();
        cfg
    }

    /// 给配置加一个填了 Key 的提供商（默认一个模型）
    fn 加提供商(cfg: &mut Config, name: &str, models: &[&str]) {
        cfg.ai.providers.insert(
            name.to_string(),
            ProviderConfig {
                base_url: "https://example.com/v4".to_string(),
                api_key: "test-key".to_string(),
                models: models.iter().map(|s| s.to_string()).collect(),
                thinking: false,
            },
        );
    }

    /// AI 提供商全部缺 Key 时**不能让传统兜底失效**。
    ///
    /// 这是「`pory -b ai` 之后整个工具不能用」那个老 bug 的守门测试的直系后代：
    /// 当年主后端构建失败用 `?` 直接返回，保底链根本没跑。现在的形态从结构上
    /// 杜绝了它 —— 缺 Key 的提供商根本不进链，传统链永远候着。
    #[test]
    fn ai_提供商全缺_key_时链落传统() {
        let cfg = 配置_ai(&["zhipu"]);
        assert!(cfg.ai.providers.is_empty(), "本测试的前提是没配提供商");

        let chain = build_mode_chain(&cfg, Mode::Ai).expect("传统兜底可用，不该报错");
        assert!(
            chain.iter().all(|b| !b.name().starts_with("zhipu")),
            "缺 Key 的不进链"
        );
        assert_eq!(chain[0].name(), "msedge", "传统默认序的第一位");
        assert_eq!(chain.len(), 4, "传统四家全在");
    }

    /// 有可用提供商时：实例名 = 提供商:模型，传统全链跟在后面兜底
    #[test]
    fn ai_可用时链首为实例_传统随后() {
        let mut cfg = 配置_ai(&["zhipu"]);
        加提供商(&mut cfg, "zhipu", &["glm-4.7-flash"]);

        let chain = build_mode_chain(&cfg, Mode::Ai).unwrap();
        assert_eq!(chain[0].name(), "zhipu:glm-4.7-flash");
        assert_eq!(
            chain.iter().map(|b| b.name()).collect::<Vec<_>>(),
            vec![
                "zhipu:glm-4.7-flash",
                "msedge",
                "transmart",
                "mymemory",
                "google"
            ]
        );
    }

    /// **一个提供商多个模型 → 展开为多个实例**，按 models 声明顺序；
    /// 同提供商的备选模型排在下一家提供商之前
    #[test]
    fn 一个提供商多个模型按序展开() {
        let mut cfg = 配置_ai(&["zhipu", "siliconflow"]);
        加提供商(&mut cfg, "zhipu", &["glm-4.7-flash", "glm-4.5-flash"]);
        加提供商(&mut cfg, "siliconflow", &["THUDM/glm-4-9b-chat"]);

        let chain = build_mode_chain(&cfg, Mode::Ai).unwrap();
        assert_eq!(
            chain.iter().map(|b| b.name()).take(3).collect::<Vec<_>>(),
            vec![
                "zhipu:glm-4.7-flash",
                "zhipu:glm-4.5-flash",
                "siliconflow:THUDM/glm-4-9b-chat",
            ],
            "同提供商的备选模型先于下一家提供商"
        );
    }

    /// models 为空的提供商不可用（没模型可调），跳过并警告
    #[test]
    fn models_为空的提供商跳过() {
        let mut cfg = 配置_ai(&["empty", "zhipu"]);
        加提供商(&mut cfg, "empty", &[]);
        加提供商(&mut cfg, "zhipu", &["m"]);

        let chain = build_mode_chain(&cfg, Mode::Ai).unwrap();
        assert_eq!(chain[0].name(), "zhipu:m");
    }

    /// 同一提供商在 order 里重复列出 → 视为一次，模型不重复展开
    #[test]
    fn ai_order_重复提供商只展开一次() {
        let mut cfg = 配置_ai(&["zhipu", "zhipu"]);
        加提供商(&mut cfg, "zhipu", &["m"]);

        let chain = build_mode_chain(&cfg, Mode::Ai).unwrap();
        assert_eq!(chain.iter().filter(|b| b.name() == "zhipu:m").count(), 1);
    }

    /// order 列了名字但没写配置表 → 跳过，不影响其余链
    #[test]
    fn order_提到未配置的提供商时跳过() {
        let mut cfg = 配置_ai(&["ghost", "zhipu"]);
        加提供商(&mut cfg, "zhipu", &["m"]);

        let chain = build_mode_chain(&cfg, Mode::Ai).unwrap();
        assert_eq!(chain[0].name(), "zhipu:m", "ghost 被跳过，zhipu 顶上");
    }

    /// provider 名字不合法（白名单外）→ 跳过，不让脏名字进缓存键
    #[test]
    fn provider_名字非法时跳过() {
        let cfg = 配置_ai(&["ZHIPU"]);
        let chain = build_mode_chain(&cfg, Mode::Ai).unwrap();
        assert!(chain.iter().all(|b| !b.name().contains("ZHIPU")));
        assert_eq!(chain[0].name(), "msedge");
    }

    /// traditional 模式下**完全不碰 AI** —— 即使提供商配置齐全
    #[test]
    fn traditional_模式不碰_ai() {
        let mut cfg = 配置_ai(&["zhipu"]);
        加提供商(&mut cfg, "zhipu", &["m"]);

        let chain = build_mode_chain(&cfg, Mode::Traditional).unwrap();
        assert_eq!(
            chain.iter().map(|b| b.name()).collect::<Vec<_>>(),
            vec!["msedge", "transmart", "mymemory", "google"]
        );
    }

    /// traditional.order 全是未知名 → 只能报错（链为空，没东西可译）
    #[test]
    fn traditional_链全错名时报错() {
        let mut cfg = Config::default();
        cfg.traditional.order = vec!["foo".to_string()];

        // 用 match 而非 unwrap_err：Box<dyn Backend> 没有 Debug，unwrap_err 编译不过
        let msg = match build_mode_chain(&cfg, Mode::Traditional) {
            Ok(_) => panic!("链为空，应当报错"),
            Err(e) => e.to_string(),
        };
        assert!(
            msg.contains("传统后端"),
            "应指明是传统列表的问题，实际：{msg}"
        );
    }

    /// order 里写重复 → 去重（同一条链里同名后端出现两次没有意义）
    #[test]
    fn traditional_链自动去重() {
        let mut cfg = Config::default();
        cfg.traditional.order = vec![
            "msedge".to_string(),
            "msedge".to_string(),
            "transmart".to_string(),
        ];

        let chain = build_mode_chain(&cfg, Mode::Traditional).unwrap();
        assert_eq!(
            chain.iter().map(|b| b.name()).collect::<Vec<_>>(),
            vec!["msedge", "transmart"]
        );
    }

    /// -b 的值只认两个模式名 —— 传统后端名不是模式名
    #[test]
    fn parse_mode_只认两个值() {
        assert_eq!(parse_mode("ai").unwrap(), Mode::Ai);
        assert_eq!(parse_mode("traditional").unwrap(), Mode::Traditional);
        assert!(parse_mode("mymemory").is_err());
        assert!(parse_mode("").is_err());
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
