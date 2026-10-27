//! Formatting, smart paste and structural diagnostic behavior.
use nozhik_config::code_style::CodeStyles;
use nozhik_language_support::{
    formatting::{php_edits, smart_paste},
    highlighter::Highlighter,
};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn formatted(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let styles = CodeStyles::builtin()?;
    let style = styles
        .selected("php", &std::collections::BTreeMap::default())
        .ok_or("missing profile")?;
    let mut output = source.to_owned();
    for (range, text) in php_edits(source, style, None)?.into_iter().rev() {
        output.replace_range(range, &text);
    }
    Ok(output)
}
#[test]
fn psr12_and_idempotence() -> TestResult {
    let source = "<?php namespace Demo;class Example{public function run($a,$b){if($a==$b){return 1+2;}}public function other(){return 3;}}";
    let output = formatted(source)?;
    assert_eq!(
        output,
        "<?php\n\nnamespace Demo;\n\nclass Example\n{\n    public function run($a, $b)\n    {\n        if ($a == $b) {\n            return 1 + 2;\n        }\n    }\n\n    public function other()\n    {\n        return 3;\n    }\n}\n"
    );
    assert_eq!(formatted(&output)?, output);
    Ok(())
}
#[test]
fn preserve_literals_comments_and_for_headers() -> TestResult {
    let source = "<?php function f(){for($i=0;$i<3;$i++){echo 'a  { b'; // ) }\n$x=[1,2];}}";
    let output = formatted(source)?;
    assert!(output.contains("'a  { b'"));
    assert!(output.contains("// ) }\n"));
    assert!(output.contains("for ($i = 0; $i < 3; $i++)"));
    assert!(output.contains("\n        $x = [1, 2];\n"));
    Ok(())
}
#[test]
fn paste_inside_method_and_no_profile() -> TestResult {
    let styles = CodeStyles::builtin()?;
    let style = styles.selected("php", &std::collections::BTreeMap::default());
    let source = "<?php\nclass C\n{\n    function f()\n    {\n        ";
    let text = "    if ($x) {\n        run();\n    }\n";
    assert_eq!(
        smart_paste(text, source, source.len(), style),
        "if ($x) {\n            run();\n        }\n"
    );
    assert_eq!(smart_paste(text, source, source.len(), None), text);
    Ok(())
}
#[test]
fn unclosed_braces_and_parentheses_ignore_strings() -> TestResult {
    let mut parser = Highlighter::php()?;
    let source = "<?php function f() { echo '}'; // }\n";
    parser.highlight(source)?;
    let diagnostics = parser.diagnostics();
    let unclosed: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.message == "Unclosed block / missing '}'")
        .collect();
    assert_eq!(unclosed.len(), 2);
    assert_eq!(unclosed[0].range.start.character, 19);
    assert_eq!(unclosed[1].range.start.line, 1);
    parser.highlight("<?php $s = '( [ {'; // {\n")?;
    assert_eq!(parser.diagnostics(), []);
    parser.highlight("<?php foo([1, 2")?;
    assert!(
        parser
            .diagnostics()
            .iter()
            .any(|d| d.message.contains("missing ']'"))
    );
    assert!(
        parser
            .diagnostics()
            .iter()
            .any(|d| d.message.contains("missing ')'"))
    );
    Ok(())
}
#[test]
fn selection_only_and_malformed_source() -> TestResult {
    let styles = CodeStyles::builtin()?;
    let style = styles
        .selected("php", &std::collections::BTreeMap::default())
        .ok_or("missing profile")?;
    let source = "<?php $a=1;$b=2;";
    let range = 6..11;
    let edits = php_edits(source, style, Some(&range))?;
    assert_ne!(edits, []);
    assert!(
        edits
            .iter()
            .all(|(r, _)| r.start >= range.start && r.end <= range.end)
    );
    assert!(php_edits("<?php function f() {", style, None).is_err());
    Ok(())
}

#[tokio::test]
async fn worker_falls_back_on_empty_lsp_for_document_and_selection() -> TestResult {
    use nozhik_language_support::{Command, Event, LanguageService, ServerCommand, lsp_types::Url};
    let dir = tempfile::tempdir()?;
    let binary = dir
        .path()
        .join(format!("formatter-lsp{}", std::env::consts::EXE_SUFFIX));
    let output = std::process::Command::new("rustc")
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/server.rs"))
        .arg("-o")
        .arg(&binary)
        .output()?;
    assert!(output.status.success());
    let log = dir.path().join("formatting.jsonl");
    let service = LanguageService::with_server(
        dir.path().into(),
        Some(ServerCommand {
            program: binary,
            args: vec![log.to_string_lossy().into_owned()],
        }),
        || {},
    );
    let uri = Url::from_file_path(dir.path().join("test.php")).map_err(|()| "URI")?;
    let source = "<?php class Sample {\npublic function test() {\n$this->doSomething();\n}\n}";
    let styles = CodeStyles::builtin()?;
    let style = styles
        .selected("php", &std::collections::BTreeMap::new())
        .ok_or("profile")?;
    assert!(service.send(Command::Sync {
        uri: uri.clone(),
        revision: 5,
        text: ropey::Rope::from_str(source)
    }));
    for range in [None, Some(6..source.len())] {
        assert!(service.send(Command::Format {
            token: 1,
            uri: uri.clone(),
            revision: 5,
            source: source.into(),
            style: style.clone(),
            range
        }));
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                for event in service.drain() {
                    if let Event::Formatted {
                        revision, edits, ..
                    } = event
                    {
                        assert_eq!(revision, 5);
                        let mut output = source.to_owned();
                        for (range, text) in edits?.into_iter().rev() { output.replace_range(range, &text); }
                        assert!(output.contains("\n    public function test()\n    {\n        $this->doSomething();\n    }\n}"));
                        return Ok::<(), Box<dyn std::error::Error>>(());
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await??;
    }
    service.shutdown();
    let log = std::fs::read_to_string(log)?;
    assert!(log.contains("textDocument/formatting"));
    assert!(log.contains("textDocument/rangeFormatting"));
    assert!(log.contains("\"tabSize\":4"));
    Ok(())
}

#[test]
fn sample_method_call_and_single_line_paste() -> TestResult {
    let source = "class Sample {\npublic function test() {\n$this->doSomething();\n}\n}";
    assert_eq!(
        formatted(source)?,
        "class Sample\n{\n    public function test()\n    {\n        $this->doSomething();\n    }\n}\n"
    );
    let styles = CodeStyles::builtin()?;
    let style = styles.selected("php", &std::collections::BTreeMap::new());
    let source = "<?php\nclass C\n{\n    function f()\n    {\n        \n    }\n}";
    let line = source.find("        \n").ok_or("line")?;
    for caret in [line, line + 3, line + 8] {
        let (range, text) = nozhik_language_support::formatting::paste_edit(
            source,
            caret..caret,
            "$user->save();",
            style,
        );
        let mut output = source.to_owned();
        output.replace_range(range, &text);
        assert!(output.contains("\n        $user->save();\n"), "{output}");
    }
    let broken = "if ($ready) {\n$x->save();\n             if ($again) {\n $x->save();\n }\n}";
    assert_eq!(
        smart_paste(broken, source, line, style),
        "        if ($ready) {\n            $x->save();\n            if ($again) {\n                $x->save();\n            }\n        }"
    );
    assert_eq!(
        smart_paste("echo '{';\nrun(); // }", source, line, style),
        "        echo '{';\n        run(); // }"
    );
    assert_eq!(
        nozhik_language_support::formatting::paste_edit(source, line..line, "$user->save();", None),
        (line..line, "$user->save();".into())
    );
    Ok(())
}

#[tokio::test]
async fn nonempty_lsp_edits_have_priority() -> TestResult {
    use nozhik_language_support::{Command, Event, LanguageService, ServerCommand, lsp_types::Url};
    let dir = tempfile::tempdir()?;
    let binary = dir
        .path()
        .join(format!("formatter-lsp{}", std::env::consts::EXE_SUFFIX));
    let output = std::process::Command::new("rustc")
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/server.rs"))
        .arg("-o")
        .arg(&binary)
        .output()?;
    assert!(output.status.success());
    let service = LanguageService::with_server(
        dir.path().into(),
        Some(ServerCommand {
            program: binary,
            args: vec![
                dir.path()
                    .join("packets.jsonl")
                    .to_string_lossy()
                    .into_owned(),
                "format-edit".into(),
            ],
        }),
        || {},
    );
    let uri = Url::from_file_path(dir.path().join("test.php")).map_err(|()| "URI")?;
    let source = "<?php $x=1;";
    let styles = CodeStyles::builtin()?;
    let style = styles
        .selected("php", &std::collections::BTreeMap::new())
        .ok_or("profile")?;
    assert!(service.send(Command::Sync {
        uri: uri.clone(),
        revision: 1,
        text: ropey::Rope::from_str(source)
    }));
    assert!(service.send(Command::Format {
        uri,
        revision: 1,
        token: 2,
        source: source.into(),
        style: style.clone(),
        range: None
    }));
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            for event in service.drain() {
                if let Event::Formatted { edits, .. } = event {
                    let mut output = source.to_owned();
                    for (range, text) in edits?.into_iter().rev() {
                        output.replace_range(range, &text);
                    }
                    assert_eq!(output, "<?php /* LSP */ $x=1;");
                    return Ok::<(), Box<dyn std::error::Error>>(());
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    service.shutdown();
    Ok(())
}

#[test]
fn selected_method_call_restores_indent_without_reformatting_its_neighbors() -> TestResult {
    let source = "<?php class Sample {\npublic function test() {\n$this->doSomething();\n}\n}";
    let start = source.find("$this").ok_or("call")?;
    let end = start + "$this->doSomething();".len();
    let styles = CodeStyles::builtin()?;
    let style = styles
        .selected("php", &std::collections::BTreeMap::new())
        .ok_or("style")?;
    let mut output = source.to_owned();
    for (range, text) in php_edits(source, style, Some(&(start..end)))?
        .into_iter()
        .rev()
    {
        assert!(range.start >= start && range.end <= end);
        output.replace_range(range, &text);
    }
    assert_eq!(output, source.replacen("$this", "        $this", 1));
    let literal = "echo \"first\n    second\";".to_owned();
    let target = "        ";
    assert_eq!(
        nozhik_language_support::formatting::paste_edit(target, 3..3, &literal, Some(style)),
        (3..3, literal)
    );
    Ok(())
}

#[test]
fn typed_print_inside_if_regains_eight_space_indent() -> TestResult {
    let source = "function check($condition, $message) {\n    if (!$condition) {\nprint('test');\n        throw new RuntimeException($message);\n    }\n}";
    assert_eq!(
        formatted(source)?,
        "function check($condition, $message)\n{\n    if (!$condition) {\n        print('test');\n        throw new RuntimeException($message);\n    }\n}\n"
    );
    Ok(())
}

#[test]
fn php82_smoke_formats_php_and_preserves_ini_payload() -> TestResult {
    let source = include_str!("fixtures/php82-smoke.php");
    let output = formatted(source)?;
    assert!(output.contains("\n        print('aaa');\n"));
    assert!(output.contains("function check($condition, $message)\n{"));
    let start = source.find("<<<INI").ok_or("heredoc opening")?;
    let end = source[start..].find("\nINI").ok_or("heredoc closing")? + start + "\nINI".len();
    assert!(output.contains(&source[start..end]));
    assert_eq!(formatted(&output)?, output);
    Ok(())
}

#[test]
fn heredoc_nowdoc_and_quoted_text_are_opaque() -> TestResult {
    let source = "<?php function emit($temporary,$x){\nfile_put_contents($temporary.'/conf/app.ini', <<<INI\n[app]\n  literal = {if ($x) {  keep   spaces }}\nkey = $x\nINI);\necho <<<'HTML'\n<div>  <?php if($x){echo 'keep';} ?>\n    Привет { }\nHTML;\necho \"two  spaces\\n { braces }\";\n}";
    let output = formatted(source)?;
    for literal in [
        "<<<INI\n[app]\n  literal = {if ($x) {  keep   spaces }}\nkey = $x\nINI",
        "<<<'HTML'\n<div>  <?php if($x){echo 'keep';} ?>\n    Привет { }\nHTML",
        "\"two  spaces\\n { braces }\"",
    ] {
        assert!(output.contains(literal), "literal changed: {literal}");
    }
    assert!(output.contains("\n    file_put_contents("));
    assert!(output.contains("\n    echo "));
    assert_eq!(formatted(&output)?, output);
    let styles = CodeStyles::builtin()?;
    let style = styles
        .selected("php", &std::collections::BTreeMap::new())
        .ok_or("style")?;
    let start = source.find("[app]").ok_or("payload")?;
    assert_eq!(php_edits(source, style, Some(&(start..start + 5)))?, []);
    Ok(())
}

#[test]
fn embedded_php_formats_without_changing_markup_or_tag_boundaries() -> TestResult {
    let source = "  <section>\n  <?php if($ok){\necho 'keep  me';\n} ?>  \n<strong>  unchanged </strong>\n<?php $x=1; ?>\n  </section>  ";
    let output = formatted(source)?;
    assert!(output.starts_with("  <section>\n  <?php "));
    assert!(output.contains("if ($ok) {\n    echo 'keep  me';\n}"));
    assert!(output.contains("?>  \n<strong>  unchanged </strong>\n<?php "));
    assert!(output.ends_with("?>\n  </section>  "));
    assert_eq!(formatted(&output)?, output);
    Ok(())
}

#[test]
fn flexible_nowdoc_indentation_and_crlf_payload_stay_exact() -> TestResult {
    let source =
        "<?php\nfunction f(){\n$x=<<<'TXT'\r\n    { keep  spaces }\r\n    TXT;\nprint($x);\n}";
    let output = formatted(source)?;
    assert!(output.contains("<<<'TXT'\r\n    { keep  spaces }\r\n    TXT"));
    assert!(output.contains("\n    print($x);\n"));
    assert_eq!(formatted(&output)?, output);
    let styles = CodeStyles::builtin()?;
    let style = styles
        .selected("php", &std::collections::BTreeMap::new())
        .ok_or("style")?;
    assert!(php_edits("<?php echo <<<MISSING\npayload\n", style, None).is_err());
    Ok(())
}
