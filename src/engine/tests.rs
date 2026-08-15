use super::*;

fn fmt(s: &str) -> String {
    match format(s.as_bytes()) {
        Some(out) => String::from_utf8(out).unwrap(),
        None => s.to_string(),
    }
}

macro_rules! changed {
    ($input:expr, $expected:expr) => {
        assert_eq!(fmt($input), $expected, "input: {:?}", $input)
    };
}

macro_rules! unchanged {
    ($input:expr) => {
        let input: &str = &$input;
        assert_eq!(fmt(input), input, "input should be unchanged: {:?}", input)
    };
}

#[test]
fn basic_cjk_latin() {
    changed!("中文English混排", "中文 English 混排");
    changed!("使用useState管理状态", "使用 useState 管理状态");
    changed!("hello世界", "hello 世界");
    changed!("这是一个test", "这是一个 test");
}

#[test]
fn basic_cjk_digit() {
    changed!("我有3个苹果", "我有 3 个苹果");
    changed!("版本v1.0发布", "版本 v1.0 发布");
    changed!("增长100%", "增长 100%");
    changed!("圆周率约3.14", "圆周率约 3.14");
}

#[test]
fn already_spaced() {
    unchanged!("中文 English 混排");
    unchanged!("我有 3 个苹果");
    unchanged!("我有  3  个苹果"); // multiple spaces are left alone
    unchanged!("中文\tenglish");
    unchanged!("中文\nenglish");
    unchanged!("中文\r\nenglish");
}

#[test]
fn pure_single_script() {
    unchanged!("hello world, this is a test.");
    unchanged!("这是一段纯中文文本，没有任何混排。");
    unchanged!("");
    unchanged!("12345 678");
}

#[test]
fn fullwidth_punctuation_neutral() {
    unchanged!("中文，english");
    unchanged!("english，中文");
    unchanged!("中文。english");
    unchanged!("「中文」test");
    unchanged!("test「中文」");
    unchanged!("中文、english");
    unchanged!("（中文）test");
}

#[test]
fn halfwidth_trailing_punct_attaches() {
    // Trailing halfwidth punctuation attaches to the preceding word; the
    // boundary space goes after the punctuation run.
    changed!("中文,english", "中文, english");
    changed!("english,中文", "english, 中文");
    changed!("中文;.english", "中文;. english");
    changed!("C++语言", "C++ 语言");
    changed!("100%增长", "100% 增长");
    changed!("增长100%", "增长 100%");
    changed!("圆周率约3.14", "圆周率约 3.14");
    // Already spaced: nothing to do.
    unchanged!("中文, english");
    unchanged!("english, 中文");
    // `!` is excluded: it would split `![alt](url)` images from preceding
    // text. `@` and `/` stay blocking (addresses, paths).
    unchanged!("中文!english");
    unchanged!("user@中文");
    unchanged!("中文/path/to");
}

#[test]
fn inline_code() {
    changed!("在这个`myfunc`函数内", "在这个 `myfunc` 函数内");
    unchanged!("在这个`中文`函数内");
    // Boundary is decided by the adjacent interior char: left edge sees 中
    // (CJK) so no space; right edge sees `d` (Latin) so a space is inserted.
    changed!("调用`中文method`结果", "调用`中文method` 结果");
    // Interior of code spans is never touched.
    unchanged!("`中文code中文`");
    changed!("使用``code`with`tick``完成", "使用 ``code`with`tick`` 完成");
    // Unclosed backtick is treated as a literal character.
    unchanged!("中文`code");
    unchanged!("a ` b");
    // Empty span: nothing to decide, stay conservative.
    unchanged!("中文``中文");
}

#[test]
fn fenced_code_block_untouched() {
    let input = "前文\n```rust\nlet s = \"中文test混排\";\n```\n后文";
    unchanged!(input);
}

#[test]
fn links() {
    changed!(
        "这是一个[link](https:Xxx)格式",
        "这是一个 [link](https:Xxx) 格式"
    );
    unchanged!("这是一个[中文链接](https://example.com)格式");
    // Latin before a CJK link text: space goes outside the bracket only,
    // never between `[` and the text (would alter the rendered link text).
    changed!("什么p[好呢](hq)还有", "什么 p [好呢](hq)还有");
    // Link text is prose and gets formatted inside.
    changed!("[中文code中文](url)", "[中文 code 中文](url)");
    // URL content never influences the outer boundary.
    unchanged!("[中文](https://english-url.com)格式");
    changed!("[link](https://url)格式", "[link](https://url) 格式");
    // Reference-style / plain brackets are opaque atoms: whether `[note]`
    // is a shortcut reference link or a literal bracket annotation cannot
    // be decided locally, so — like parens and quotes — the brackets hug
    // their content and never create boundaries.
    unchanged!("中文[note]english");
    unchanged!("中文[note]中文");
    unchanged!("中文[中文]english");
    // Inline links keep their text-decided boundaries.
    changed!("中[中文](url)b", "中[中文](url) b");
    changed!("中**中文**b", "中**中文** b");
    changed!("中`中文`b", "中`中文` b");
    // Malformed: space inside parens -> not a URL, treated as plain text;
    // the `)` is opaque and blocks the boundary.
    unchanged!("[a](b c)中文");
    // Nested parens inside URL (Wikipedia style).
    changed!(
        "见[link](https://en.wiki/Foo_(bar))条目",
        "见 [link](https://en.wiki/Foo_(bar)) 条目"
    );
    // Images: leading `!` blocks the left boundary (v1 limitation).
    changed!("![alt](url)中文", "![alt](url) 中文");
    unchanged!("中文![alt](url)");
}

#[test]
fn emphasis_and_quotes() {
    changed!("这是*important*一点", "这是 *important* 一点");
    changed!("这是**bold**一点", "这是 **bold** 一点");
    changed!("这是~~deleted~~一点", "这是 ~~deleted~~ 一点");
    unchanged!("这是*重点*一点");
    // Quotes and parens are opaque: they block the boundary, so markup is
    // never split from the text it wraps.
    unchanged!("他说\"hello\"了");
    unchanged!("他说'hello'了");
    unchanged!("他说\"你好\"了");
    unchanged!("中文(english)中文");
    unchanged!("中文(中文)中文");
}

#[test]
fn curly_quotes() {
    unchanged!("他说“hello”了");
    unchanged!("他说“你好”了");
}

#[test]
fn other_scripts() {
    changed!("これはtestです", "これは test です");
    changed!("한국어test입니다", "한국어 test 입니다");
    // Fullwidth alphanumerics count as CJK, so no boundary with 汉字.
    unchanged!("使用ＡＢＣ排版");
    changed!("ＡＢＣabc", "ＡＢＣ abc");
}

#[test]
fn emoji_and_symbols() {
    unchanged!("中文🎉english");
    unchanged!("中文🎉中文");
}

#[test]
fn boundary_at_edges() {
    changed!("中文a", "中文 a");
    changed!("a中文", "a 中文");
    unchanged!("中文");
    unchanged!("a");
    unchanged!("中");
}

#[test]
fn idempotent() {
    let cases = [
        "中文English混排",
        "在这个`myfunc`函数内",
        "这是一个[link](https:Xxx)格式",
        "我有3个苹果",
        "这是*important*一点",
        "他说“hello”了",
        "galgame CG**鉴赏**",
        "<span title=\"你知道的太多了\">测试text</span>",
        "<!-- 注释comment -->后文",
        "负面消息[^1][^2]让我",
        "解法([ref](url))：",
    ];
    for case in cases {
        let once = fmt(case);
        let twice = fmt(&once);
        assert_eq!(once, twice, "not idempotent: {case:?}");
    }
}

#[test]
fn malformed_utf8_passthrough() {
    let input = b"abc\xE4\xB8 def \xFF\xFEghi";
    assert!(format(input).is_none());
    // CJK adjacent to malformed byte: malformed byte blocks the boundary.
    let mut v = "中文".as_bytes().to_vec();
    v.push(0xff);
    v.extend_from_slice(b"abc");
    assert!(format(&v).is_none());
}

#[test]
fn malformed_run_tails() {
    // A stray continuation byte at a multibyte run tail must keep the
    // conservative per-char semantics (no space stuffed across it).
    let mut v = "中".as_bytes().to_vec();
    v.push(0x80);
    v.extend_from_slice(b"a");
    assert!(format(&v).is_none(), "input: {v:?}");
    // Run of pure continuation bytes.
    assert!(format(&[0x80, 0x80, b'a']).is_none());
    // Truncated lead byte at end of input.
    assert!(format(&[0xe4, 0xb8]).is_none());
    // A standalone malformed byte inside a run: the run layout stays valid,
    // so the surrounding CJK/Latin boundaries still apply.
    let mut w = b"a".to_vec();
    w.extend_from_slice("中".as_bytes());
    w.push(0xff);
    w.extend_from_slice("文".as_bytes());
    w.extend_from_slice(b"b");
    let expected: &[u8] = b"a \xe4\xb8\xad\xff\xe6\x96\x87 b";
    assert_eq!(format(&w), Some(expected.to_vec()));
}

#[test]
fn long_multibyte_runs() {
    // Runs longer than one SWAR word exercise the SIMD continuation.
    let long = "中".repeat(64);
    assert_eq!(fmt(&format!("{long}a")), format!("{long} a"));
    assert_eq!(fmt(&format!("a{long}")), format!("a {long}"));
    // Fullwidth punctuation inside long runs stays neutral; the final CJK
    // char still bounds against the trailing Latin char.
    let mixed = "中，文，字".repeat(20);
    assert_eq!(fmt(&format!("{mixed}x")), format!("{mixed} x"));
    // 4-byte codepoints (emoji) in runs never create boundaries.
    let emoji = "🎉".repeat(16);
    assert_eq!(fmt(&format!("a{emoji}b")), format!("a{emoji}b"));
}

#[test]
fn crlf_and_mixed_lines() {
    changed!("第一行line1\r\n第二行line2", "第一行 line1\r\n第二行 line2");
}

#[test]
fn long_ascii_run_performance_shape() {
    // Mostly-ASCII document with a single boundary at the end.
    let mut s = "lorem ipsum dolor sit amet ".repeat(500);
    s.push_str("中文end");
    let expected = s.replacen("中文end", "中文 end", 1);
    assert_eq!(fmt(&s), expected);

    // No boundary at all -> unchanged.
    let pure = "lorem ipsum ".repeat(10_000);
    unchanged!(&pure);
}

#[test]
fn dense_boundaries() {
    changed!("a中b文c字d", "a 中 b 文 c 字 d");
    changed!("中a文b字c", "中 a 文 b 字 c");
}

#[test]
fn markdown_documents() {
    changed!("# 标题title\n正文content。", "# 标题 title\n正文 content。");
    unchanged!("## 标题\n\n正文，标点 english 混排。");
    changed!("- 列表item1\n- 列表item2", "- 列表 item1\n- 列表 item2");
    // Hard-coded strings in md code spans are protected.
    unchanged!("配置项 `name中文` 保持原样");
    changed!("配置项`name中文`保持", "配置项 `name中文`保持");
}

#[test]
fn autolinks_tables_and_reference_definitions() {
    // Autolinks are opaque atoms, like HTML tags.
    unchanged!("见<https://example.com/x>网站");
    unchanged!("见<user@example.com>网站");
    // Table cells are prose; the pipes block.
    changed!("| 甲a | 乙b |", "| 甲 a | 乙 b |");
    // Reference definitions: the `[id]` marker is opaque (bare brackets),
    // the URL is plain text.
    unchanged!("[1]: https://example.com");
}

#[test]
fn indented_code_block_untouched() {
    // Indented code after a blank line: hardcode stays untouched.
    unchanged!("上文\n\n    let s = \"中文test混排\";\n\n下文");
    // Prose around it is still formatted; multiple indented lines and
    // interior blank lines belong to the same block.
    changed!(
        "上文a\n\n    中文test代码\n\n    第二行x代码\n\n下文b",
        "上文 a\n\n    中文test代码\n\n    第二行x代码\n\n下文 b"
    );
    // Tab-indented line.
    unchanged!("上\n\n\t中文test混排\n下");
    // Lazy continuation: an indented line that does NOT follow a blank line
    // is paragraph text, not code.
    changed!(
        "段落\n    缩进continues中文",
        "段落\n    缩进 continues 中文"
    );
    // Indented code opening the document.
    unchanged!("    中文test代码\n\n正文");
    // An indented blank line is not code.
    changed!("上文x\n    \n下文y", "上文 x\n    \n下文 y");
}

#[test]
fn front_matter_untouched() {
    // YAML front matter is metadata: hardcoded strings stay untouched,
    // the body is formatted.
    changed!(
        "---\ntitle: 中文test混排\n---\n正文content",
        "---\ntitle: 中文test混排\n---\n正文 content"
    );
    // CRLF line endings.
    changed!(
        "---\r\ntitle: a中文b\r\n---\r\n正文x",
        "---\r\ntitle: a中文b\r\n---\r\n正文 x"
    );
    // `...` also closes the block (YAML document end marker).
    unchanged!("---\nkey: 中文x值\n...\n");
    // No closing fence: not front matter (just a thematic break), the rest
    // is prose.
    changed!("---\n中文test", "---\n中文 test");
    // Not at document start: not front matter.
    changed!("\n---\n中文test", "\n---\n中文 test");
}

// The following test groups persist real-world regression cases: constructs
// where adding a space would change rendering or break semantics.

#[test]
fn html_tags_opaque() {
    // Attribute values are never inspected.
    unchanged!("<span class=\"heimu\" title=\"你知道的太多了\">不过我已经关了自动更新</span>");
    unchanged!("<a title=\"中文english混合\">x</a>");
    // No space between a tag edge and adjacent CJK text.
    unchanged!("<heimu>我 PC 端剪贴板</heimu>");
    unchanged!("<dtlslong>装了 Debian 10</dtlslong>");
    unchanged!("<summary>查看</summary>");
    unchanged!("<div class=\"subtitle\">记录点点滴滴</div>");
    unchanged!("<h3>定位精确的软件</h3>");
    unchanged!("<text style=\"color:red;font-weight:bold\">未解决！</text>以下功能默认为免费版");
    unchanged!("她拿了<span>奖学金</span>。10 至 11 月");
    unchanged!("（问题）</span>然后因为");
    unchanged!("数据<span class=\"heimu\">隐藏</span>内容");
    unchanged!("grub 引导<span class=\"x\">注</span>");
    // Self-closing tags block both sides.
    unchanged!("崩溃<br/>![空指针](x.png)");
    unchanged!("便宜<br/>延迟低");
    unchanged!("存疑<br/>来源请求");
    // Comments: the body is prose and gets formatted, but the comment
    // blocks boundaries on both sides.
    changed!("<!-- 注释comment -->", "<!-- 注释 comment -->");
    changed!("<!-- 注释x>注释y中文z -->", "<!-- 注释 x>注释 y 中文 z -->");
    unchanged!("中文<!--注释-->中文");
    unchanged!("<!-- 注释 -->后文");
    // Non-comment declarations stay opaque.
    unchanged!("<!DOCTYPE 中文x系统>");
    // Unterminated comment: `<!--` falls back to plain text (the CJK/Latin
    // boundary inside still applies, since it is no longer a comment).
    changed!("<!-- 未闭合注释comment", "<!-- 未闭合注释 comment");
    unchanged!("中文<!-- 未闭合");
    // A `<` that does not open a valid tag is a literal char.
    unchanged!("a < b 并且 c > d");
    // The literal `<` blocks its own seam, but the `2|中` boundary has no
    // markup at it, so the usual rule applies.
    changed!("1<2中文", "1<2 中文");
}

#[test]
fn vue_and_custom_tags() {
    // Slot names must never be split.
    unchanged!("<template #廃村少女2>");
    unchanged!("<template #9nine九次九日九重色>");
    unchanged!("<template #弹丸论破2>");
    unchanged!("<template #兰斯01重制>");
    unchanged!("<template #天使☆嚣嚣RE-BOOT!>");
    unchanged!("<template #春音AliceGram>");
}

#[test]
fn furigana_and_badge() {
    // Furigana attributes keep CJK content intact; tags never split words.
    unchanged!("<furigana f=\"ワードプロセッサ\">word</furigana>");
    unchanged!("|体|<furigana f=\"たい\">体</furigana>育|");
    unchanged!("「…<furigana f=\"たち\">質</furigana>が悪い」");
    // An unpaired `**` followed by a tag: no space is inserted.
    unchanged!("GB 数**<Badge type=\"tip\" text=\"合理价格\" />");
}

#[test]
fn footnote_refs() {
    unchanged!("leetcode 上的[^1]题目");
    unchanged!("负面消息[^1][^2]让我");
    unchanged!("“三分饥[^2]和寒”");
    // Footnote definitions: the marker is opaque, the body is prose.
    changed!("[^1]: 参考reference文献", "[^1]: 参考 reference 文献");
    // `[^text]` followed by `(` is an ordinary link whose text happens to
    // look like a footnote; the right boundary is decided by the link text
    // (the leading `[` still blocks the left boundary).
    changed!("见[^top](https://url)条目", "见[^top](https://url) 条目");
}

#[test]
fn parens_and_quotes_block() {
    // No space is ever stuffed inside parens, even around links/code.
    unchanged!("解法([ref](url))：");
    unchanged!("顶层模块(`crate`)");
    unchanged!("中文(english)中文");
    // Quotes hug their content; no boundary is created through them.
    unchanged!("拒绝断定“A 和 B 是同一个人”");
    unchanged!("喊“UNO!”");
    unchanged!("Luna“算法稳定币”");
    unchanged!("“打开一个 PDF”模式");
    unchanged!("重命名为”damage.cfg”");
    // The apostrophe inside a word is not at the CJK boundary, so the usual
    // rule applies to the word as a whole.
    changed!("it's中文", "it's 中文");
}

#[test]
fn emphasis_pairing() {
    // Spaces go outside the markers, decided by the interior content.
    changed!("galgame CG**鉴赏**", "galgame CG **鉴赏**");
    changed!("bold**中文**后缀", "bold **中文**后缀");
    changed!(
        "前缀**中文english混排**后缀",
        "前缀**中文 english 混排**后缀"
    );
    // Unpaired markers stay transparent.
    unchanged!("星号*不成对中文");
    changed!("5*3中文的结果", "5*3 中文的结果");
    // List bullets are not emphasis openings.
    unchanged!("* 列表项 item");
    // Mismatched run lengths do not pair; the transparent `*` is looked
    // through as before.
    changed!("前缀*em**后缀", "前缀 *em** 后缀");
}

#[test]
fn emphasis_pairing_window() {
    // Pairs inside the window behave normally.
    let inner = "中".repeat(100); // 300 bytes < MAX_EMPHASIS_SPAN
    changed!(
        &format!("a**{inner}**b"),
        format!("a **{inner}** b").as_str()
    );
    // Interior beyond the window: the opener pairs optimistically with the
    // far closer, so spaces still go OUTSIDE the markers — inserting them
    // inside (`a** {far} **b`) would destroy the delimiters' flanking and
    // disable the emphasis in the rendered output.
    let far = "中".repeat(2000); // 6000 bytes > MAX_EMPHASIS_SPAN
    let input = format!("a**{far}**b");
    let expected = format!("a **{far}** b");
    assert_eq!(fmt(&input), expected);
    assert_eq!(fmt(&expected), expected, "not idempotent: {input:?}");
}

#[test]
fn emphasis_never_pairs_into_code_spans() {
    // Regression: a stray `*` in prose used to pair with the `*` inside a
    // later code span, desyncing the backtick pairing and stuffing spaces
    // INSIDE the following spans (`匹配` aabbbbc``).
    unchanged!(
        "乘法用*表示。一阶段：`ab*c` 匹配 `aabbbbc`；二阶段：`aa*a` 匹配 `baab`；boss \
         战：`a.*b.+c` 匹配 `cababbcbc`。"
    );
    changed!(
        "用*表。`ab*c`匹配`aabbbbc`。",
        "用*表。`ab*c` 匹配 `aabbbbc`。"
    );
    // A `~~` in prose must not pair with one inside a span either.
    changed!("划掉~~再说`a~~b`看", "划掉~~再说 `a~~b` 看");
    // A `*` inside an HTML tag attribute is markup, not a closer.
    unchanged!("重点*事项<img alt=\"*\">图");
}

#[test]
fn emphasis_wrapping_code_spans() {
    // Emphasis legitimately containing a code span: the wrapper's boundary
    // is decided by the span's interior content.
    changed!(
        "一阶段：*`ab*c`*匹配`aabbbbc`。",
        "一阶段：*`ab*c`* 匹配 `aabbbbc`。"
    );
    changed!("中文**`code`x**结尾", "中文 **`code`x** 结尾");
    changed!("中**`代码`文**尾", "中**`代码`文**尾");
}

#[test]
fn link_text_with_code_spans() {
    // Code spans inside link text decide the link's boundary by their
    // interior; the span itself is never touched.
    changed!("见[`c`](u)文", "见 [`c`](u) 文");
    unchanged!("见[`中文`](u)文");
}

#[test]
fn pathological_emphasis_input_stays_fast() {
    // Regression guard for the pairing window: without it, every `*` run
    // rescans the rest of the input (O(n^2)). Sized to stay quick in debug
    // builds while still being painful without the window.
    let input = "*a ".repeat(10_000);
    unchanged!(&input);
}
