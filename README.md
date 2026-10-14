# zhfmt

简体中文 | [English](./README.en.md)

高性能中文文档批量格式化工具：遵循中文写作原则（盘古之白），在中文与英文/数字之间自动添加空格（盘古之白）。

- 安全：只插入单个空格，从不删除任何内容；原子化替换文件。
- 快速：SIMD（AVX2/SSE2，运行时检测）+ SWAR 混合扫描 + 多字节跳跃 + CoW，无任何正则；多文件并行处理。
- 专注：不同于 autocorrect，zhfmt 只做加空格这一件事，尽量不产生预期外的修改。

该工具主要用于 markdown 文档格式化，适配 markdown 链接、内嵌 html、inline code、代码块、脚注格式、转义，尽力避免非预期的行为。也可以用于其他纯文本格式的空格添加。

## 安装

前往 [Release](https://github.com/lxl66566/zhfmt/releases) 下载 prebuilt binary。

## 使用

```sh
zhfmt                       # 格式化当前目录下所有文档类文件（递归）
zhfmt docs/ README.md       # 格式化指定路径（目录递归，文件总是处理）
zhfmt --check               # CI 模式：只报告会变化的文件，存在差异时退出码为 1
zhfmt --diff                # 打印 unified diff，不写回，退出码同上
cat a.md | zhfmt            # 管道模式：stdin -> stdout
zhfmt --ext md,rst docs/    # 覆盖要处理的扩展名列表
zhfmt -j 4                  # 指定线程数
zhfmt --schema              # 输出 zhfmt.json 的 JSON Schema
```

- 默认只格式化以下扩展名文件：`md, markdown, mdx, txt, rst`；显式传入的文件路径不受限制。
- 文件扫描遵循 `.gitignore`。
- exit code：0 正常；1 `--check`/`--diff` 发现差异；2 错误。

效果示例：

```text
在这个`myfunc`函数内           ->  在这个 `myfunc` 函数内
这是一个[link](https:Xxx)格式  ->  这是一个 [link](https:Xxx) 格式
我有3个苹果                    ->  我有 3 个苹果
```

更多边界样例（inline code、链接、强调、引号、日/韩文、畸形 UTF-8 等）请直接阅读 [src/engine/tests.rs](src/engine/tests.rs)。

### 作为 prettier 插件使用

[`prettier-plugin-zhfmt`](./prettier-plugin-zhfmt) 将引擎编译为 WASM（128-bit SIMD）并包装 prettier 内置 markdown parser，在保留 prettier 原生格式化的同时补上中英文空格：

```sh
pnpm add -D prettier-plugin-zhfmt
```

```json
// In your prettier.config.js
{ "plugins": ["prettier-plugin-zhfmt"] }
```

更多用法详见[插件文档](./prettier-plugin-zhfmt/README.md)。

## 配置文件

查找顺序：从当前目录逐级向上查找 `zhfmt.json` 或 `.zhfmt.json`；fallback 全局配置（`$XDG_CONFIG_HOME/zhfmt/zhfmt.json`，Windows 下为 `%APPDATA%` 对应目录）。

```json
{
  "$schema": "https://raw.githubusercontent.com/lxl66566/zhfmt/main/zhfmt.schema.json",
  "extensions": ["md", "markdown", "mdx", "txt", "rst"],
  "include": ["docs/*.adoc"],
  "exclude": ["node_modules/", "target/"],
  "whitelist": ["B站", "3D"],
  "jobs": 0
}
```

- `$schema`：JSON Schema 地址，供编辑器补全与校验，zhfmt 忽略该字段；schema 也可用 `zhfmt --schema` 在本地生成
- `extensions`：要处理的扩展名白名单，整体覆盖默认值
- `include`：额外的 glob 白名单，命中后即使扩展名不匹配也会处理
- `exclude`：glob 黑名单，相对路径匹配
- `whitelist`：分词白名单，每条恰好两个字符，表示这两个字符之间不插入空格（如 `"B站"` 保持 `B站` 不加空格）
- `jobs`：处理线程数，0 或 null 为自动

## 算法

核心在 [src/engine](src/engine)，字符分类在 [src/classify.rs](src/classify.rs)，完整设计见 [docs/design.md](docs/design.md)。

## 性能

criterion 基准（`cargo bench`）。本机结果：Linux x86_64 / AVX2，AMD 7945HX，
32 线程，多轮代表值。

| 场景                      | 输入大小        | 吞吐 / 耗时 |
| ------------------------- | --------------- | ----------- |
| 纯 ASCII                  | ~0.89 MiB       | ~43.7 GiB/s |
| 纯中文                    | ~1.03 MiB       | ~74.9 GiB/s |
| 混合中英文档              | ~0.68 MiB       | ~1.27 GiB/s |
| 已格式化文本              | ~0.33 MiB       | ~1.10 GiB/s |
| 混合文本 1KB–4MB          | ~1.5 KB–5.97 MB | ~700 MiB/s  |
| 256 文件端到端（--check） | 256 × ~16 KiB   | ~3.3 ms     |

## License

MIT OR Apache-2.0
