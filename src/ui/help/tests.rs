use super::*;

#[test]
fn tiny_help_rows_should_fit_without_losing_description_characters() {
    let rendered = lines(vec![("j".into(), "abcdefghij".into())], 10);

    assert_eq!(
        rendered.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["j", "  abcdefgh", "  ij"]
    );
}

#[test]
fn wide_help_keys_should_keep_descriptions_in_the_aligned_column() {
    let rendered = lines(
        vec![
            ("a key wider than the column".into(), "wide".into()),
            ("C-c".into(), "quit".into()),
        ],
        60,
    );

    let column = |line: &Line| line.to_string().find(|c: char| c.is_alphabetic());
    assert_eq!(column(&rendered[1]), Some(22));
    assert_eq!(rendered[2].to_string().find("quit"), Some(22));
}
