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
fn halfwidth_punctuation_blocks() {
    unchanged!("中文,english");
    unchanged!("中文!english");
    unchanged!("C++语言");
    unchanged!("100%增长");
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
    // Link text is prose and gets formatted inside.
    changed!("[中文code中文](url)", "[中文 code 中文](url)");
    // URL content never influences the outer boundary.
    unchanged!("[中文](https://english-url.com)格式");
    changed!("[link](https://url)格式", "[link](https://url) 格式");
    // Reference-style / plain brackets.
    changed!("中文[note]english", "中文 [note]english");
    changed!("中文[note]中文", "中文 [note] 中文");
    unchanged!("中文[中文]english");
    // Malformed: space inside parens -> not a URL, treated as plain text.
    changed!("[a](b c)中文", "[a](b c) 中文");
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
    changed!("他说\"hello\"了", "他说 \"hello\" 了");
    changed!("他说'hello'了", "他说 'hello' 了");
    unchanged!("他说\"你好\"了");
    changed!("中文(english)中文", "中文 (english) 中文");
    unchanged!("中文(中文)中文");
}

#[test]
fn curly_quotes() {
    changed!("他说“hello”了", "他说 “hello” 了");
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
    changed!("中文", "中文");
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
