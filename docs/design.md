# zhfmt 设计文档

本文档描述 zhfmt 的总体架构、核心算法与 SIMD 扫描器设计，以及性能测试方法与结果。

## 1. 总体架构

```
src/
├── lib.rs        # crate 根：导出 format / format_str
├── classify.rs   # 字符分类（Class 模型 + ASCII 表 + UTF-8 解码）
├── engine/       # 核心引擎（纯函数，无 IO）
│   ├── mod.rs    # Formatter 状态机：主循环 + on_* 事件处理器
│   ├── scan.rs   # 字节扫描层：混合 SWAR/SIMD find_wake / find_ascii + Scan 运行时选择
│   ├── boundary.rs # 边界判定层：crosses + 双向类查询 + 结构构造扫描（HTML/URL/强调）
│   └── tests.rs  # 引擎单测与回归用例
├── config.rs     # zhfmt.json 配置发现与解析（bin only）
├── process.rs    # 并行遍历 + IO + 模式（write/check/diff）（bin only）
└── main.rs       # CLI（clap derive）
```

引擎内部按职责分三层：`scan` 负责"下一个有趣字节在哪"（纯字节搜索，
不含任何边界语义）；`boundary` 负责"某个接缝两侧的有效字符类是什么、
某个构造延伸到哪"（纯函数）；`mod` 的 `Formatter` 状态机把两者粘合，
持有 `prev`/`pos`/`last` 状态并实现各构造的事件处理器——handlers 之间
共享状态，是必须整体阅读的决策核心，因此保持在一个文件内。

- 引擎与 IO 彻底分离：`engine::format(&[u8]) -> Option<Vec<u8>>` 是纯函数，
  无文件系统、无锁、无全局状态，天然可并行且易于测试。
- 运行时依赖仅 `memchr`；CLI 相关依赖（clap/ignore/globset/memmap2 等）全部由 `bin` feature 门控。

数据流：`ignore` 并行遍历 → 每文件读取（小文件 `read`，大文件 mmap）→ 引擎格式化 →
`None` 则跳过写回；`Some` 则按模式 report / diff / 原子替换。

## 2. 字符分类（classify.rs）

每个字符被归入七个类之一，类之间的组合关系决定了空格的插入：

| Class | 成员 | 边界语义 |
|---|---|---|
| `Latin` | `a-zA-Z0-9` | 与 `Cjk` 相邻时插入空格 |
| `Cjk` | 汉字（含扩展 A-F）、假名、谚文、注音、全角字母数字、半角片假名等 | 同上 |
| `Neutral` | 全角/中文标点（`，` `。` `「」` …） | 永不产生边界 |
| `Soft` | 未配对的 `*` `~` | 透明：判定时被透视 |
| `Hard` | `` ` `` `[` `]` | 结构界定符，唤醒扫描器 |
| `Space` | ASCII 空白 | 打断任何边界 |
| `Other` | 其余 ASCII 符号、引号、括号、裸 `<>`、emoji、畸形字节 | 阻断边界（不透明） |

实现要点：

- ASCII 字节走 128 项查表（`ASCII_CLASS`）；多字节字符按需解码（`decode_char`），
  畸形序列返回 `(U+FFFD, 1)`，归类为 `Other` 原样透传——二进制安全。
- codepoint → Class 是纯 `const fn` 的区间匹配（`classify_codepoint`）。
- 关键不变量：**`Latin` 与 `Soft` 一定是 ASCII**。这是多字节 run 跳跃（§4）正确性的基石。

## 3. 边界模型

扫描器维护 `prev: Option<Class>`（最近一个有效内容字符的类）。插入空格当且仅当：

```
crosses(prev, next) = (Latin, Cjk) | (Cjk, Latin)
```

- 前向判定 `peek_forward_class` / `next_is_latin`：跳过 `Soft`，遇 `Space`/`Hard`/EOF 返回无。
- 后向判定 `lookback_class`：跳过 `Soft`；`Hard` 或区域起点意味着该边界已被先前事件处理，保留传入 `prev`。
- 引号、括号、emoji 等一律 `Other` 阻断——标记与所包裹文本之间永远不会被塞入空格。

结构构造（code span / 链接 / HTML / 脚注 / 强调）的边界语义见 §6。

## 4. 扫描器设计

### 4.1 wake byte

插入只可能发生在「非 ASCII 字节」或「结构界定符」处，其余字节可以整块跳过。
定义 wake byte 为：

```
b >= 0x80 || b in { '`', '[', ']', '<', '*', '~' }
```

`<`/`*`/`~` 不是边界字符，但可能开启 HTML 标签 / 强调 run，需要唤醒扫描器处理。

### 4.2 混合扫描：内联 SWAR + SIMD 续扫

找到下一个 wake byte 是引擎最频繁的操作。设计为两层：

**第一层（内联 SWAR，免调用开销）**：以 u64 word 为单位，
用经典 zero-byte 检测（`(x - 0x0101…) & !x & 0x8080…`）同时探测
高位字节与 6 个结构字节。`find_wake` 内联检查头 **2 个 word（16 字节）**，
命中则在 word 内逐字节定位；`find_ascii`（找 run 终点）同理。

**第二层（SIMD 续扫，长距离摊薄）**：头 16 字节都是"无聊"字节时，
交给运行时检测选出的长距离扫描器：AVX2（32B/步）或 SSE2（16B/步，x86_64 基线），
其他架构回退 SWAR 循环。

为什么必须是混合而不是纯 SIMD：

- Windows x64 ABI 中 ymm6–15/xmm6–15 是 callee-saved。SIMD 扫描函数若用到
  超过 6 个向量寄存器，每次调用都要付出 4–5 次 XMM 保存/恢复 + `vzeroupper`
  的函数序言（实测约 30–50 cycles）。
- 混合中英文本文本中，两次事件（run 边界、wake byte）的典型间距只有约 10 字节，
  纯 SIMD 方案"序言开销 > 扫描收益"，实测反而比 SWAR 慢。
- 因此短距离用内联 SWAR 免调用；SIMD 只在确认有 ≥16 字节的干净区段后才接管，
  序言成本被 32B/步的吞吐摊薄。2 word 的分界点是实测调优的结果。

SIMD 细节（`engine::x86`）：

- wake 探测：对 6 个结构字节各做一次 `cmpeq` 后 OR，再与 `movemask(c)`
  的符号位（天然标记所有 `>= 0x80` 字节）在 GPR 域合并，`tzcnt` 定位首个命中。
- 非法字节安全的对齐无关读取（`read_unaligned`）。
- 运行时检测用 `is_x86_feature_detected!("avx2")`，结果经 `OnceLock` 缓存为
  函数指针对（`Scan`），`Formatter` 每次运行取一次，循环内无分支。
- 已知残留：即使把比较常量放进 `.rodata` 以内存操作数引用，LLVM 仍会把
  broadcast 提升到循环外，长距离 AVX2 版本在 Windows 上仍带有序言。
  这被有意接受——它只在长干净区段上执行。

### 4.3 多字节 run 跳跃

`on_multibyte` 的关键观察：`Latin` 与 `Soft` 都是 ASCII，因此
**两个多字节字符之间永远不可能产生边界**。连续多字节字符构成一个 *run*：

1. `find_ascii` 找到 run 终点（下一个 ASCII 字节），run 内部整体跳过——
   不再逐字符解码 + 分类 + 前后窥探。
2. 只分类 run 的**首字符**（且仅当 `prev == Latin` 时才需要，否则跳过这次解码）
   与**尾字符**（可能参与右侧边界）。
3. 右侧边界用 `next_is_latin` 判定：跳过 `*`/`~` 后检查一个字节是否为
   ASCII 字母数字，等价于 `peek_forward_class(...) == Some(Latin)` 但零解码。
4. `prev = Some(尾字符类)`，`pos = run 终点`，继续主循环。

畸形布局回退：从 run 终点回溯续字节得到尾字符起点，若 `尾字符起点 + 解码长度 != 终点`
（例如尾部有游离续字节），说明 run 布局非法，回退到逐字符处理
（`on_multibyte_char`），保证坏字节仍按 `Other` 单字节步进、阻断边界。

该优化对纯中文文本是数量级提升：整段中文现在只需 2 次解码 + 一次 SIMD 扫描。

## 5. 输出策略：COW + 纯插入

- 输出缓冲在**第一个插入点**才惰性分配（预分配 `len + len/8 + 16`）。
  已格式化好的文件只花一次线性扫描，零分配、零写回。
- 变换是纯插入式的（只插入单个空格，从不删除/改写），保证幂等；
  幂等性有专门测试（`idempotent`）。

## 6. 结构构造的边界语义

| 构造 | 处理 | 边界 |
|---|---|---|
| code span `` `x` `` | 内部原样跳过（含嵌套反引号配对） | 由内部首/尾内容字符决定 |
| 链接 `[t](url)` | text 按正文，url 整体跳过（容忍一层嵌套括号） | 由 text 决定，url 不影响 |
| HTML `<…>` / `<!--…-->` / `<?…>` | 不透明原子，含属性值（`title="中文"`、`<template #槽位>`）永不扫描 | 两侧一律阻断 |
| 脚注 `[^id]` | 不透明原子 | 两侧一律阻断 |
| 强调 `*…*`/`**…**`/`~~…~~` | 成对（含 crude CommonMark 左右 flank 检查，拒绝列表符）时作为包装：内部递归格式化。**配对搜索原子地跳过 code span 与 HTML 标签/注释**——标记内部的 `*`/`~` 是代码或标记语言，绝不参与配对（否则配对跳转会落入 span 内部，使后续反引号配对整体错位，把空格插进 code span） | 空格只加在标记**外侧**，由内部内容决定（内部含 code span 时由 span 内部首/尾字符决定） |
| 未配对 `*`/`~` | `Soft` 透明 | 透视 |

所有构造在无法配对/畸形时都退化为保守的字符语义，不会误改。

## 7. 进程层（process.rs）

- **遍历**：`ignore::WalkBuilder` 并行遍历，`hidden(false)` 使 dotfile 也被扫描，
  默认遵循 gitignore；exclude 用 `OverrideBuilder`，include 用 `globset`。
  显式传入的文件绕过扩展名过滤，并按 canonicalize 路径去重防止双写。
- **单次打开**：`File::open` 一次，`file.metadata()` 取句柄元数据——
  相比按路径 `metadata` + `read` 省掉一次路径解析（Windows 上昂贵）。
- **读取策略**：`< 256 KiB` 预分配并 `read_to_end`；`>= 256 KiB` 只读 mmap
  （unix 上 `Advice::Sequential`）。
- **写回**：`None`（无变化）永不写；有变化经同目录 `NamedTempFile` + `persist`
  原子替换，保留权限。Windows 上写回前先 `drop(file)` 句柄，
  否则 rename-over-open-handle 会失败。

## 8. 性能测试

方法：criterion，`cargo bench --bench single_thread`（引擎吞吐）与
`cargo bench --bench multi_files`（端到端，256 文件 × ~16 KiB，check 模式）。

结果（Linux x86_64 / AVX2，AMD Zen 5，`--sample-size 20` 多轮代表值。

| 场景 | 输入大小 | 吞吐 / 耗时 |
|---|---|---|
| 纯 ASCII | ~0.89 MiB | ~43.7 GiB/s |
| 纯中文 | ~1.03 MiB | ~74.9 GiB/s |
| 混合中英文档 | ~0.68 MiB | ~1.27 GiB/s |
| 已格式化 | ~0.33 MiB | ~1.10 GiB/s |
| 混合文本 1KB–4MB | ~1.5 KB–5.97 MB | ~700 MiB/s |
| 256 文件端到端（check） | 256 × ~16 KiB | ~3.3 ms |

历史对比（Windows x86_64 / AVX2 开发机，相对 SIMD 化 + run 跳跃之前的
实现 SWAR 8B/步 + 逐字符多字节处理）：纯 ASCII ~4.7x、混合中英文档 ~2.3x、
纯中文 ~350x+、已格式化 ~2.3x、256 文件端到端 ~1.7x。

优化来源拆解：

1. 多字节 run 跳跃：消除逐字符解码/分类/双向窥探（纯中文、长中文段的主要收益）。
2. `next_is_latin` 免解码前窥（混合文本高事件密度场景的主要收益）。
3. SIMD 续扫 + 内联 SWAR 混合：干净 ASCII 区段与长中文 run 的扫描带宽。
4. 单次 open + 惰性分配：端到端 IO 与分配开销。

## 9. 已知限制（v1）

- fenced code block 的感知依赖行内无 wake 结构的巧合（``` 围栏内的内容
  若含 wake byte 仍会被扫描）；计划 v2 引入行级状态。
- 图片 `![alt](url)` 的 `!` 阻断左边界。
- 强调配对是"crudely CommonMark"：只做左右 flank 的空白检查，
  不做完整的 delimiter stack 算法。
