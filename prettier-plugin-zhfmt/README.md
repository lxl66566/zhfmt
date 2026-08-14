# prettier-plugin-zhfmt

简体中文 | [English](./README.en.md)

[Prettier](https://prettier.io) 插件：在中文与英文/数字之间自动添加空格（盘古之白），补上 Prettier 3 移除该功能后的空缺。格式化引擎是 [zhfmt](https://github.com/lxl66566/zhfmt) 编译的 WebAssembly，与 CLI 行为完全一致。

## 安装

```sh
pnpm add -D prettier-plugin-zhfmt
```

要求 Prettier >= 3.6 (peer dependency)。更早的版本不会 await parser 的 `preprocess`，会导致输出为空。

## 使用

`.prettierrc`：

```json
{
  "plugins": ["prettier-plugin-zhfmt"]
}
```

无需其他配置。插件会**包装** Prettier 内置的 `markdown` / `mdx` parser：先用 zhfmt 加空格，再交给 Prettier 自己的解析与打印。因此 Markdown 文件仍保留 Prettier 全部原生格式化能力（列表、表格、proseWrap、嵌入代码格式化等），只是额外获得中英文空格：

### 纯文本模式

对不想要 Prettier 重排、只要加空格的文件（`.txt`、`.rst` 等无内置 parser 的扩展名自动推断；其他文件可显式指定），使用独立 parser `zhfmt`：除插入空格外输出与输入逐字节一致（不增删换行、不改列表标记）：

```json
{
  "plugins": ["prettier-plugin-zhfmt"],
  "overrides": [{ "files": ["*.txt"], "options": { "parser": "zhfmt" } }]
}
```

## 行为说明

- 只插入单个空格，从不删除任何内容；幂等。
- 代码块、inline code、链接 URL、HTML 标签内部不加空格。
- 详见 [zhfmt 引擎边界规则](https://github.com/lxl66566/zhfmt#算法简述)。

## License

MIT OR Apache-2.0
