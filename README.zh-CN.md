# Pory

**在终端里直接翻译 —— 开箱免 Key，也能接上你自己选的 AI。不用开浏览器，不用复制粘贴。**

`pory` 是一个给常驻命令行的人用的小翻译器。装完即用，记得住翻过的东西，
并且**如实告诉你这次的结果到底是谁翻的**。

> 名字来自[多边兽 Porygon](https://zh.wikipedia.org/wiki/%E5%A4%9A%E8%BE%B9%E5%85%BD) ——
> 一只由数据构成、能适配不同环境的生物。翻译也是这件事：把一种符号系统转成另一种。

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

```console
$ pory "今天天气不错"
The weather is nice today
◈ zh-CN ⇄ en · msedge · 1 block · cache 0/1 · 0.5s
```

上面那行 `◈` 是唯一的装饰，它说清了：哪两种语言、用了哪个后端、切了几块、
有几块来自缓存、整次调用花了多久 —— 而它走 **stderr**，所以把 stdout 重定向出去，
得到的只有译文。

## 特性

- **两种模式，一个参数** —— `ai`（大模型：质量最好、懂上下文）与 `traditional`
  （免 Key 机翻：最快）。`ai` 模式会把传统链接在后面当保底，所以没 Key、被限流
  只是降级，不会罢工。`-b ai` / `-b traditional` 按次切换
- **零配置可用** —— 没有任何配置文件也能跑：传统链首选 `msedge`，免 Key、国内直连
- **后端可插拔** —— 四个免 Key 传统后端，外加任何 OpenAI 兼容接口（DeepSeek、Kimi、
  智谱、硅基流动、本地 Ollama…）。一个提供商可配多个模型：主力模型挂了，
  先试同家的备选，再轮到下一家
- **自动回退，且看得见** —— 没 Key、超额、超时都会切到下一个后端，翻译不中断。
  会有一条警告说明发生了什么，脚注里显示真实路径 —— 静默回退是不诚实的，
  那会让你以为在用大模型，实际拿到的是机翻结果
- **AI 思考开关** —— `thinking = false` 关掉混合推理模型的思考阶段。
  硅基流动实测：Qwen3-8B 从 22.9 秒降到 1.4 秒，质量无损
- **第一 / 第二语言互翻** —— 配好母语和第二语言后，auto 模式下原文已是母语的，
  就改译第二语言，而不是原样丢回来
- **shell 集成** —— `pory --init-shell fish` 装上短名（`pr` → `pory`）与补全，
  `bash` / `zsh` 同样支持
- **干净卸载** —— `pory --uninstall` 清掉它加的一切（补全文件、rc 里追加的行、缓存），
  只有配置文件会先问一次
- **管道友好** —— `cat notes.md | pory -t en`。颜色、脚注和动画只在对应流是终端时出现，
  管道、CI、重定向里一个字节都不多写
- **长文本智能切分** —— 段落 → 句子 → 硬切三级降级，不腰斩语义
- **按块缓存** —— 上次翻过的一句话这次零成本，命中约 4 ms
- **小而快** —— 单个静态二进制（约 3.7 MB），启动约 3 ms，无运行时依赖
- **纯 Rust + rustls** —— 不依赖 OpenSSL，交叉编译到 musl / ARM 不用折腾系统库

## 安装

```bash
cargo install --git https://github.com/harukizmoe/pory --locked
```

`--locked` 让依赖固定在仓库 `Cargo.lock` 记录的版本上 —— 免得日后某个依赖发新版，把安装搞坏。

或从源码构建：

```bash
git clone https://github.com/harukizmoe/pory
cd pory
cargo build --release        # 二进制在 target/release/pory
```

## 快速上手

```bash
pory "今天天气不错"            # -> The weather is nice today
pory "hello world"             # -> 你好，世界        （auto：英文进、中文出）
pory "Good morning" -t ja      # -> おはようございます
pory "你好，世界" -t en         # -> Hello, world.
pory "こんにちは" -f ja -t zh   # 明确指定源语言和目标语言
cat notes.md | pory -t en      # 管道输入
pory "hello" --plain           # 只要译文，不要脚注和动画
pory -b traditional "hello"    # 完全绕开 AI，直接用机翻
```

默认 `primary = "zh"`、`secondary = "en"`，所以前两条不加任何参数就能双向工作。
改配置即可让任意一对语言都这样。

## 用法

```
Usage: pory [OPTIONS] [TEXT]
```

| 参数 | 说明 |
|---|---|
| `[TEXT]` | 要翻译的文本。省略则从 stdin 读取（支持管道） |
| `-t, --target <LANG>` | 目标语言，如 `zh` / `en` / `ja` |
| `-f, --from <LANG>` | 源语言，默认 `auto` 自动检测 |
| `-b, --backend <MODE>` | `ai` / `traditional`。**它有两种含义，见下** |
| `--plain` | 只输出纯译文：不加颜色、不要脚注、不要动画 |
| `--no-cache` | 本次不使用缓存（不读也不写） |
| `--refresh` | 忽略已有缓存，强制重新翻译并更新缓存 |
| `--clear-cache` | 清空翻译缓存并退出 |
| `--init` | 生成示例配置文件并退出（已存在时绝不覆盖） |
| `--init-shell <SHELL>` | 安装 shell 集成：`fish` / `bash` / `zsh` |
| `--print` | 配合 `--init-shell`：只打印脚本，不安装 |
| `--uninstall` | 清掉 pory 在这台机器上加的所有东西，然后退出 |

### `-b` 的两种含义

区别只在**有没有输入**：

```bash
pory -b traditional "text"           # 这一次走传统链
echo "text" | pory -b traditional    # 同上 —— 管道输入也算「这一次」
pory -b traditional                  # 没有文本 → 写进配置，以后每次都用它
```

最后这条会打印 `✓ 已把默认翻译模式设为 traditional` 并退出，**只改配置里 `mode` 那一行**，
其余内容和注释原样保留。

如果 `-b` 和别的参数一起出现但没有文本（`pory -b ai -t ja`），pory 会打印用法提示
而不是擅自改配置 —— 那种情况多半是忘了给文本。

### 支持的语种

`zh` `zh-cn` `zh-tw` `en` `ja` `ko` `fr` `de` `es` `ru` `it` `pt` `ar` `th` `vi`

### shell 集成

```bash
pory --init-shell fish     # -> ~/.config/fish/completions/pory.fish
pory --init-shell bash     # -> 补全文件 + 往 ~/.bashrc 追加 `alias pr=pory`
pory --init-shell zsh      # -> 打印脚本 + 往 ~/.zshrc 追加同样的别名
```

它给你两样东西：短名（敲 `pr` + 空格即展开成 `pory`）和补全（语种、模式、参数）。
所有改动都是幂等的、带标记的，所以 `pory --uninstall` 能整块撤掉；
不是 pory 写的文件，绝不会被覆盖。

两个细节值得知道：

- `pr` 与系统的 POSIX 分页工具 `/usr/bin/pr` 同名。在交互式 shell 里它展开成 `pory`；
  要用真的 `pr` 就敲 `\pr` 或 `command pr`
- zsh 没有补全文件的标准位置（取决于你 `$fpath` 怎么配），所以 zsh 只打印脚本 +
  告诉你怎么放。zsh 的补全是按规范写的但**未在真实 zsh 上验证**；fish 和 bash 都实跑验证过

### 卸载

```bash
pory --uninstall
```

清掉补全文件、`~/.bashrc` / `~/.zshrc` 里追加的那几行、以及翻译缓存 —— 这些都不问，
因为都是可再生的。配置文件是另一回事：里面可能有你填的 API Key 和调过的设置，
所以 pory 会先问一次（`[y/N]`）。非交互环境下（脚本里跑）一律保留配置。

二进制本身不会被删除（它可能由 cargo 或包管理器管着），命令会告诉你怎么删。

## 配置

**全部是可选的**。运行 `pory --init` 生成
`~/.config/pory/config.toml`（Windows 在 `%APPDATA%\pory\`）：

```toml
mode = "ai"                 # ai | traditional
primary = "zh"              # 第一语言（母语）：不指定 -t 时翻成它
secondary = "en"            # 第二语言：与第一语言互翻，留空 "" 即关闭
source = "auto"
cache = true                # false 等价于每次都带 --no-cache

# ── AI 提供商（OpenAI 兼容，可配多个）──
[ai]
order = ["zhipu", "siliconflow"]   # 按此顺序尝试；没填 api_key 的自动跳过

[ai.siliconflow]
base_url = "https://api.siliconflow.cn/v1"
api_key = ""
models = ["tencent/Hunyuan-MT-7B", "Qwen/Qwen3-8B"]
thinking = false            # 可选：请求体带 enable_thinking = false

# ── 传统机翻（免 Key，永远可用）──
# order 同时决定 mode = "traditional" 时的链顺序，和 mode = "ai" 时接在 AI 后面的保底顺序。
[traditional]
order = ["msedge", "transmart", "mymemory", "google"]
```

找不到配置文件就用内置默认值，**不会报错**。缓存同理：读不到或文件损坏时，
pory 会静默降级为空缓存，绝不会因为缓存出问题就让工具不能用。

### 第一语言 / 第二语言互翻

**auto 模式下**（既不写 `-f` 也不写 `-t`），pory 按检测出的原文语言自动选方向：

```bash
pory "hello world"     # 原文是英文 → 译成中文
pory "你好，世界"       # 原文已是中文 → 改译成英文
```

这样就不会出现「中文翻中文」这种没有意义的请求。三条边界：

- **显式 `-t` 时永不换方向** —— 你指明了目标就照做
- **显式 `-f` 时也不换** —— 源语言已知；源和目标相同时本地直接返回，不发请求
- **`secondary` 留空即关闭互翻**

这不是白来的魔法 —— **每个后端都得自己处理它**，而且各家协议不同：

- **MyMemory**：服务端回 `translatedText: null` 加一个检测语种，pory 拿备用目标再发一次请求
- **AI 后端**：写进 prompt ——「翻成 X；若原文已经是 X，则改译成 Y」，一次往返搞定，不用检测请求
- **msedge / transmart / google**：放任不管的话它们会老老实实「把中文翻成中文」、
  把原文当译文退回（实测）。pory 从服务端自报的语种、或「译文与原文完全相同」判断出这种情况，
  再用备用目标重发一次 —— 只在 auto 模式下

## 后端

| 后端 | 需要 Key | 国内直连 | 说明 |
|---|---|---|---|
| `msedge` | ✗ | ✓ | 微软 Edge 端点。默认传统链的第一家 |
| `transmart` | ✗ | ✓ | 腾讯交互翻译。实测最快（约 0.2 秒） |
| `mymemory` | ✗ | ✓ | 1000 次/天，单次 400 字符；填邮箱能提额度 |
| `google` | ✗ | ✗ | 质量好，非官方接口，国内需代理，有风控风险 |
| `ai` | ✓ | 看服务商 | 质量最好、懂上下文、保留格式。比机翻慢且要花钱 |

一个 AI 提供商就是 `[ai.名字]` 一节，可以带多个模型；链里展开成「提供商:模型」，
所以失败时能指名道姓说是哪个模型。协议是 **OpenAI 兼容**，换 `base_url` 和 `models` 即可：

| 服务商 | `base_url` |
|---|---|
| DeepSeek | `https://api.deepseek.com/v1` |
| Kimi | `https://api.moonshot.cn/v1` |
| 智谱 | `https://open.bigmodel.cn/api/paas/v4` |
| 硅基流动 | `https://api.siliconflow.cn/v1` |
| 本地 Ollama | `http://localhost:11434/v1` |

所有后端都带 **30 秒请求超时**。没有它，连接一旦挂起就会等到内核 TCP 超时
（Linux 约 130 秒），看起来就是「卡住」；而且保底后端也轮不上，
因为回退发生在上一个后端**返回失败**之后。

## 输出

```
↳ hello world                                       ← 原文回显（只在管道 / 文件输入时）
你好世界                                             ← 译文（stdout，终端里加粗）
◈ zh-CN ⇄ en · msedge · 10 blocks · cache 9/10 · 1.0s   ← 脚注（stderr，始终贴在屏底）
```

脚注是这套界面里最诚实的部分：

- `⇄` 表示方向按检测结果定；`›` 表示方向确定
- `bad:m → sf:tencent/Hunyuan-MT-7B` 表示**发生过回退**，此时整行转琥珀色
- `10 blocks` 是真实的切块数，在发出任何请求之前就本地算好了
- `cache 9/10` 是「10 块里 9 块命中缓存」
- `1.0s` 是整次调用的墙钟耗时，含切块与缓存落盘。缓存命中显示 `0.0s` ——
  那是如实报告「瞬时」，不是四舍五入的假象

联网期间会有一行盲文 thinking 动画，尾随**实测**秒数 —— 不做进度条，
因为我们真的不知道一次网络请求要多久。

完整的取舍说明，以及排版背后的字形宽度实测：[docs/output.md](docs/output.md)。

## 设计说明

缓存为什么用 JSON 而不是 SQLite、缓存键为什么用 SHA-256、tokio 为什么跑单线程、
装饰为什么走 stderr，以及双模式路由怎么设计的：
[docs/design.md](docs/design.md) 与 [docs/mode-routing-design.md](docs/mode-routing-design.md)。

## 参与开发

欢迎提 issue 和 PR。几条能让补丁更容易被接受的习惯：

- `cargo test` 和 `cargo clippy --all-targets` 都要干净
- 源码注释用中文；`--help` 文本**只用英文**（它是用户界面，本项目刻意只保留一种语言）
- 新功能必须 **opt-in**：不配置时，代码路径与行为要和加这个功能之前完全一致

## 许可证

[MIT](LICENSE)
