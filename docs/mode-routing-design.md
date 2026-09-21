# 双模式路由设计与实现方案

状态：**已实施**（2026-09-21 定稿并落地；71 测试全绿 · clippy 零警告 · 端到端五路径实测通过）
前置调研：`docs/research-immersive-translate.md` · 免费后端实测记录（2026-09-21 日志）

> 实施落地时的增量决策（定稿后新增）：
> 1. **一个提供商对应多个模型**：`model: String` → `models: Vec<String>`，
>    链的基本单位细化为「提供商×模型」实例，实例名 `zhipu:glm-4.7-flash`；
>    展开顺序 = order 优先级 + 提供商内 models 声明顺序（同提供商的备选模型先于下一家）。
> 2. `Ai` 实例持有动态名字（`new(name, base_url, key, model)`），
>    脚注回退路径能分清是哪个模型挂了；缓存键 = 组合名 + model，双保险。
> 3. `Mode` 实现 `Display`（显示为配置文件里的写法 ai / traditional）。

---

## 0. 一句话

把翻译模式收敛为 **`ai`** 与 **`traditional`** 两类；mode 只是**构建后端链的策略**，
链本身仍是现有 `Translator` 的一条平链 —— 调度、回退、缓存、脚注机制**零改动**。

用户已拍板：不兼容旧配置字段（`backend` / `fallback` 直接废弃）。

---

## 1. 最终配置形态

```toml
# 翻译模式：ai | traditional。默认 ai。
mode = "ai"

# ── AI 模式：OpenAI 兼容提供商，可配多个 ──
[ai]
# 按顺序尝试；没填 api_key 的自动跳过
order = ["zhipu"]

[ai.zhipu]
base_url = "https://open.bigmodel.cn/api/paas/v4"
model = "glm-4.7-flash"
api_key = ""

# ── 传统模式：免 Key 机翻 ──
[traditional]
# 默认顺序：微软（质量最好、国内直连）→ 腾讯交互翻译（最快、直连）
# → MyMemory（现默认，1000 次/天）→ Google（需代理、有风控，排末位）
order = ["msedge", "transmart", "mymemory", "google"]
```

要点：
- `[ai]` 下既有 `order` 键又有若干子表，靠 serde `flatten` 收集：
  显式字段吃掉 `order`，其余键全部落进 `providers: HashMap<String, ProviderConfig>`。
  TOML 写法就是自然的 `[ai.zhipu]`，**不需要** `[ai.providers.zhipu]` 多一层。
- provider 名限 `a-z 0-9 -`（会进缓存键与脚注）。仅要求 TOML 表名自身合法 +
  字符白名单校验，无保留字问题（v2 中 AI provider 与传统后端是两个名字空间）。
- 谁配了 Key 谁参与路由；order 里写了但没配的 provider 只影响警告，不影响运行。

## 2. 路由语义（精确定义）

| 场景 | 行为 |
|---|---|
| `pory "..."`，mode=ai | 链 = `ai.order` 逐个（缺 Key 跳过）**拼接** `traditional.order` 全链 |
| `pory "..."`，mode=traditional | 链 = 仅 `traditional.order`，AI 完全不碰 |
| `pory -b ai "..."` | 链首换成 ai 模式（= 与默认 mode=ai 相同的链）。沿用现有语义：**`-b` 只改链首，兜底照走** |
| `pory -b traditional "..."` | 链 = 传统链，临时省 tokens |
| `pory -b <模式>`（无输入+终端） | 持久写 `mode = "..."`（沿用 `replace_top_level`，文本级替换保注释） |
| 某 AI provider 429 / 超时 / Key 失效 | 运行时沿链回退（translator 现有行为），脚注琥珀色显示完整实际链 |
| 所有 AI provider 缺 Key | 构建时合并成**一条**警告「AI 未配置 api_key，本次直接使用传统翻译」，不要 N 条 |
| 链为空（如 traditional.order 全是未知名） | 报错，列出传统后端的合法名字 |

设计约束（已拍板，不再讨论）：
- 传统内部顺序 `msedge → transmart → mymemory → google`；
- 缓存键跟随**实际**后端（provider 名 + model），mode 字段不进键；
- 脚注永远显示实际用到的链（回退可见、转琥珀）；
- 不做自动择优路由；不做单提供商粒度的 CLI。

## 3. 数据结构（config.rs）

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode { Ai, Traditional }

pub struct Config {
    pub mode: Mode,
    pub primary: String,          // 原样保留
    pub secondary: String,        // 原样保留
    pub source: String,           // 原样保留
    pub cache: bool,              // 原样保留
    pub mymemory_email: Option<String>,
    pub google_endpoint: Option<String>,
    pub ai: AiSection,
    pub traditional: TraditionalSection,
    // 删除：backend、fallback
}

pub struct AiSection {
    pub order: Vec<String>,
    #[serde(flatten)]
    pub providers: HashMap<String, ProviderConfig>,
}

// ProviderConfig 即现有 AiConfig（base_url / api_key / model），改名复用
pub struct TraditionalSection {
    pub order: Vec<String>,
}
```

- `Mode` 序列化为小写字符串；解析失败按现有哲学警告 + 回落默认（ai）。
- `Default`：`mode = Ai`；`ai.order = []`（无提供商）；`traditional.order =
  ["msedge","transmart","mymemory","google"]`。
- `set_backend` → `set_mode(mode)`：只改顶层 `mode = "..."` 行，机制不变。
- `template()` 重写：中文注释写清两类模式、三家免费 AI 提供商的注册地址与
  base_url 速查（智谱 / 硅基流动 / 混元）、传统四家的取舍说明。

## 4. 模块改动清单（按文件）

### 4.1 `config.rs`
- 结构体按上节改；删 `backend` / `fallback` 及相关模板行；
- `set_backend` → `set_mode`；
- 新增校验：provider 名字符白名单；order 里出现未知传统后端名 → 警告（构建时跳过）；
- 单测：flatten 解析、Mode 序列化、模板可解析、set_mode 保留注释（沿用现有
  `replace_top_level` 测试的姿势）。

### 4.2 `backend/mod.rs`
- 删 `KNOWN` 常量与 `is_known`（其三处消费点全部改型，见 4.5 / 4.6）；
- 新增传统后端的构建函数：

```rust
pub fn build_traditional(name: &str, cfg: &Config) -> Result<Box<dyn Backend>>
// match: "msedge" | "transmart" | "mymemory" | "google"，未知名报 Config 错
```

- `http_client` / `net_err` / `Request` / `Backend` trait：**零改动**。

### 4.3 `backend/ai.rs`
- `Ai` 增加字段 `name: String`；`new` 改为 `new(name, base_url, api_key, model)`；
- `fn name(&self) -> &str { &self.name }`（原来硬编码 "ai"）；
- `cache_detail` 已返回 model，不动；prompt / 请求体 / 超时，不动。

### 4.4 新文件 `backend/msedge.rs`（微软 Edge 免鉴权端点，已实测）
- `POST https://edge.microsoft.com/translate/translatetext?to=<to>&isEnterpriseClient=false`
  ，非 auto 时才追加 `&from=<from>`；
- body 为**纯字符串数组** `["原文"]`（不是 Azure 的 `[{"Text":...}]` —— 400 坑）；
- 必须带浏览器 User-Agent；单次上限约 5 万字符，`max_chars()` 取 5000；
- 响应 `[{"detectedLanguage":..., "translations":[{"text":...}]}]`，取
  `[0].translations[0].text`；HTML 风控页防护同 google.rs 的姿势；
- 语言映射：`to_microsoft()` 加在 `lang.rs`（先例是 `to_mymemory` 在那里）：
  `zh-CN → zh-Hans`、`zh-TW → zh-Hant`，其余原样；
- `to_if_same`：v1 忽略（同 google），注释写明；
- 请求体/URL 拆成纯函数便于单测；注释写明「auth 接口 2026-08 已 404，此端点
  无承诺，坏了走回退」。

### 4.5 新文件 `backend/transmart.rs`（腾讯交互翻译，已实测）
- `POST https://transmart.qq.com/api/imt`，头带 `Origin/Referer/User-Agent`；
- body：`header.fn = "auto_translation"`；`client_key` 仿浏览器指纹
  `browser-chrome-131.0.0-Windows_10-<uuid>-<毫秒时间戳>` —— **不引 uuid crate**
  （轻量铁律），用纳秒时间戳 + 地址熵拼 8-4-4-4-12 十六进制，约 20 行，注释说明；
- `source.lang` / `target.lang` 用两位码：复用 `Lang::to_mymemory()` 的逻辑即可
  （zh-CN → zh），不新增映射函数；auto 写 "auto"；
- 响应取 `auto_translation[0]`；`src_lang` 留作未来「真实语向」待办的接口；
- `max_chars()` 取 4000（网页输入框限 5000，留余量）；`to_if_same` v1 忽略。

### 4.6 `main.rs`
- `build_chain(cfg, primary_name)` → `build_mode_chain(cfg, mode: Mode)`：

```
Ai 模式：  ai.order 逐个构建（api_key 空 → 跳过并记账）
           + traditional.order 全链
           + 全部 AI 被跳过时打印一条合并警告
Traditional：traditional.order 逐个构建（未知名跳过警告）
链空 → Err，错误文案列出合法值（模式名 / 传统后端名）
```

- `try_set_backend` → `try_set_mode`：合法值 `["ai","traditional"]`（静态，无需
  读配置）；写 `mode`；「顺手确认能不能用」的提醒改为检查该模式链是否为空；
- `Cli.backend` 的 doc comment（`--help` 文本，英文）改为说明两模式语义；
- `resolve_swap` / 动画 / 输出栅格：零改动。

### 4.7 脚注与输出（print_result / footnote）
- 机制零改动。实际链自动变长（极端 `zhipu → msedge → transmart → mymemory`），
  琥珀规则不变（`backends.len() > 1` 判据依然成立）。

## 5. 缓存与 FORMAT_VERSION

- 键结构不变：`SHA-256(后端名 + cache_detail + 源 + 目标 + 备用目标 + 原文块)`；
- AI 条目从 `ai` 变为 `zhipu:<model>` 等 —— 旧 `ai` 键**自然失配**（既不命中也
  不污染，等于惰性过期），**不需要递增 FORMAT_VERSION**；mymemory / google 的旧
  缓存继续命中；
- 结论：**`FORMAT_VERSION` 不动**。

## 6. 明确不做

- 自动择优 / 按语言按长度选提供商；
- 旧 `backend` / `fallback` 字段的兼容映射（已拍板不要）；
- 单提供商粒度的 CLI（`-b zhipu` 不存在，换家改配置 order）;
- 新依赖（uuid / rand 都不引入）。

## 7. 测试计划

| 区域 | 内容 |
|---|---|
| config | flatten 解析 `[ai.zhipu]`、Mode 大小写、模板可解析、set_mode 保注释、provider 名白名单 |
| build_mode_chain | ai 全缺 Key → 单条合并警告 + 链落传统；traditional 链正确；未知名跳过；链空报错 |
| msedge / transmart | URL/body 纯函数单测、语言映射表（zh-CN→zh-Hans / zh） |
| 存量 | main.rs 里 5 个 build_chain 测试重写；footnote/切分/缓存测试不动；预计 48 → 60+ 全绿 + clippy 零警告 |

实测验收（真请求）：`pory "hello"` 走默认链（无 Key → 传统 msedge）；配置 zhipu+
Key 后走 AI；人为把 zhipu 的 base_url 改错 → 观察回退链与琥珀脚注；
`pory -b traditional` 不碰 AI；缓存二连发第二次全命中。

## 8. 实施顺序（每步可独立验收）

1. `config.rs`：新结构 + 模板 + set_mode（单测绿）
2. `lang.rs` 加 `to_microsoft`；新建 `msedge.rs` / `transmart.rs`（单测绿，
   并用 curl/PowerShell 复测端点仍活）
3. `ai.rs` name 参数化（一行改动 + 构造点）
4. `backend/mod.rs` 删 KNOWN、加 `build_traditional`；`main.rs` 换
   `build_mode_chain` + `try_set_mode` + help 文本（存量测试重写后全绿）
5. WSL 全量 `cargo test --release` + `clippy -- -D warnings`
6. 实测五条验收路径（第 7 节）
7. 文档：README 三语后端说明段 + `docs/` 本文档状态改为已实施
   （README 示例输出必须取自实测）

## 9. 环境风险（执行注意）

- 构建/测试一律 WSL：`CARGO_TARGET_DIR=~/.cache/pory-target cargo build --release`；
- 本会话 Bash 工具损坏（shim 缺 dirname）、PowerShell 无回显 →
  统一走 `wsl.exe … > 重定向到文件` + Read 读结果；端点实测用
  PowerShell 写 `\\wsl.localhost\archlinux\tmp\` 再 Read 的已验证通路。

## 10. 待确认点（动手前最后一次过目）

1. `-b ai` 失败后**仍落传统兜底**（沿用「-b 只换链首」语义）—— 若你希望
   `-b ai` 显式失败即报错、不落传统，说一声，改动很小；
2. 内部名 `msedge`（脚注显示 `msedge`）是否顺眼；备选 `edge` / `bing`；
3. AI 段字段名 `mode` vs 保留 `backend` 字段名 —— 方案按 `mode` 写。
