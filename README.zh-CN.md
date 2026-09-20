# Pory

**在终端里直接翻译 —— 不用 API Key，不用开浏览器，不用复制粘贴。**

`pory` 是一个给常驻命令行的人用的小翻译器。装完即用，记得住翻过的东西，
并且**如实告诉你这次的结果到底是谁翻的**。

> 名字来自[多边兽 Porygon](https://zh.wikipedia.org/wiki/%E5%A4%9A%E8%BE%B9%E5%85%BD) ——
> 一只由数据构成、能适配不同环境的生物。翻译也是这件事：把一种符号系统转成另一种。

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

```console
$ pory "今天天气不错"
It's a nice day today
◈ zh-CN ⇄ en · mymemory · 1 block · cache 0/1
```

上面那行 `◈` 是唯一的装饰，它说清了：哪两种语言、用了哪个后端、切了几块、
有几块来自缓存 —— 而它走 **stderr**，所以把 stdout 重定向出去，得到的只有译文。

## 特性

- **零配置可用** —— 默认后端 MyMemory 免 Key、国内直连，`cargo install` 完就能跑
- **后端可插拔** —— MyMemory / Google / 任何 OpenAI 兼容接口（DeepSeek、Kimi、智谱、
  本地 Ollama…），配置里改一行就能切
- **自动回退保底** —— 主后端没 Key、超额、超时都会自动切到备选，翻译不中断。
  会有一条警告说明发生了什么，脚注里显示真实路径 —— 静默回退是不诚实的，
  那会让你以为在用大模型，实际拿到的是机翻结果
- **第一 / 第二语言互翻** —— 配好母语和第二语言后，auto 模式下原文已是母语的，
  就改译第二语言，而不是原样丢回来
- **管道友好** —— `cat notes.md | pory -t en`。颜色和动画只在对应流是终端时出现，
  管道、CI、重定向里一个字节都不多写
- **长文本智能切分** —— 段落 → 句子 → 硬切三级降级，不腰斩语义
- **按块缓存** —— 上次翻过的一句话这次零成本，命中约 4 ms
- **小而快** —— 单个静态二进制（约 3.6 MB），启动约 3 ms，无运行时依赖
- **纯 Rust + rustls** —— 不依赖 OpenSSL，交叉编译到 musl / ARM 不用折腾系统库

## 安装

```bash
cargo install --git https://github.com/harukizmoe/pory
```

或从源码构建：

```bash
git clone https://github.com/harukizmoe/pory
cd pory
cargo build --release        # 二进制在 target/release/pory
```

## 快速上手

```bash
pory "今天天气不错"            # -> It's a nice day today
pory "hello world"             # -> 你好世界          （auto：英文进、中文出）
pory "Good morning" -t ja      # -> おはようございます
pory "你好，世界" -t en         # -> Hello, world.
pory "こんにちは" -f ja -t zh   # 明确指定源语言和目标语言
cat notes.md | pory -t en      # 管道输入
pory "hello" --plain           # 只要译文，不要脚注和动画
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
| `-b, --backend <NAME>` | `mymemory` / `google` / `ai`。**它有两种含义，见下** |
| `--plain` | 只输出纯译文：不加颜色、不要脚注、不要动画 |
| `--no-cache` | 本次不使用缓存（不读也不写） |
| `--refresh` | 忽略已有缓存，强制重新翻译并更新缓存 |
| `--clear-cache` | 清空翻译缓存并退出 |
| `--init` | 生成示例配置文件并退出 |

### `-b` 的两种含义

区别只在**有没有输入**：

```bash
pory -b google "text"           # 这一次用 google
echo "text" | pory -b google    # 同上 —— 管道输入也算「这一次」
pory -b google                  # 没有文本 → 写进配置，以后每次都用它
```

最后这条会打印 `✓ 已把默认后端设为 google` 并退出，**只改配置里 `backend` 那一行**，
其余内容和注释原样保留。改回去：`pory -b mymemory`。

如果 `-b` 和别的参数一起出现但没有文本（`pory -b google -t ja`），pory 会打印用法提示
而不是擅自改配置 —— 那种情况多半是忘了给文本。

### 支持的语种

`zh` `zh-tw` `en` `ja` `ko` `fr` `de` `es` `ru` `it` `pt` `ar` `th` `vi`

## 配置

**全部是可选的**。运行 `pory --init` 生成
`~/.config/pory/config.toml`（Windows 在 `%APPDATA%\pory\`）：

```toml
backend = "mymemory"
fallback = ["mymemory"]     # 主后端失败时依次尝试的备选
primary = "zh"              # 第一语言（母语）：不指定 -t 时翻成它
secondary = "en"            # 第二语言：与第一语言互翻，留空 "" 即关闭
source = "auto"
cache = true                # false 等价于每次都带 --no-cache

# MyMemory：填邮箱能提额度
# mymemory_email = "you@example.com"

# Google：可以填自建反代地址
# google_endpoint = "https://your-worker.workers.dev/translate_a/single"

# AI 后端（OpenAI 兼容协议）
[ai]
base_url = "https://api.deepseek.com/v1"
api_key = ""
model = "deepseek-chat"
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

性能上不多花钱：只有**撞上同语种**那一条路径才会多发一次请求。

## 后端

| 后端 | 需要 Key | 国内直连 | 说明 |
|---|---|---|---|
| `mymemory` | ✗ | ✓ | 默认。1000 次/天，单次 400 字符 |
| `google` | ✗ | ✗ | 质量好，非官方接口，国内需代理，有风控风险 |
| `ai` | ✓ | 看服务商 | 质量最好、懂上下文、保留格式。比机翻慢且要花钱 |

AI 后端只写了一个实现就能切多家服务，因为它用的是 **OpenAI 兼容协议**，
换 `base_url` 和 `model` 即可：

| 服务商 | `base_url` |
|---|---|
| DeepSeek | `https://api.deepseek.com/v1` |
| Kimi | `https://api.moonshot.cn/v1` |
| 智谱 | `https://open.bigmodel.cn/api/paas/v4` |
| 本地 Ollama | `http://localhost:11434/v1` |

所有后端都带 **30 秒请求超时**。没有它，连接一旦挂起就会等到内核 TCP 超时
（Linux 约 130 秒），看起来就是「卡住」；而且保底后端也轮不上，
因为回退发生在上一个后端**返回失败**之后。

## 输出

```
↳ hello world                                  ← 原文回显（只在管道 / 文件输入时）
你好世界                                        ← 译文（stdout，终端里加粗）
◈ zh-CN ⇄ en · mymemory · 1 block · cache 0/1   ← 脚注（stderr，始终贴在屏底）
```

脚注是这套界面里最诚实的部分：

- `⇄` 表示方向按检测结果定；`›` 表示方向确定
- `ai → mymemory` 表示**发生过回退**，此时整行转琥珀色
- `3 blocks` 是真实的切块数，在发出任何请求之前就本地算好了
- `cache 3/3` 是「3 块全部命中缓存」

联网期间会有一行盲文 thinking 动画，尾随**实测**秒数 —— 不做进度条，
因为我们真的不知道一次网络请求要多久。

完整的取舍说明，以及排版背后的字形宽度实测：[docs/output.md](docs/output.md)。

## 设计说明

缓存为什么用 JSON 而不是 SQLite、缓存键为什么用 SHA-256、tokio 为什么跑单线程、
装饰为什么走 stderr、以及怎么用「一个新文件 + 一行 match」加一个后端：
[docs/design.md](docs/design.md)。

## 参与开发

欢迎提 issue 和 PR。几条能让补丁更容易被接受的习惯：

- `cargo test` 和 `cargo clippy --all-targets` 都要干净
- 源码注释用中文；`--help` 文本**只用英文**（它是用户界面，本项目刻意只保留一种语言）
- 新功能必须 **opt-in**：不配置时，代码路径与行为要和加这个功能之前完全一致

## 许可证

[MIT](LICENSE)
