use crate::pattern::parser::*;
use maelstrom_test_runner::{maybe_and, maybe_not, maybe_or};

#[cfg(test)]
use crate::parse_str;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Case {
    pub name: String,
    pub node_id: String,
    pub markers: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Context {
    pub package: String,
    pub file: Option<String>,
    pub case: Option<Case>,
}

impl Context {
    fn file(&self) -> Option<&String> {
        self.file.as_ref()
    }

    fn case(&self) -> Option<&Case> {
        self.case.as_ref()
    }
}

pub fn interpret_simple_selector(s: &SimpleSelector) -> Option<bool> {
    use SimpleSelectorName::*;
    Some(match s.name {
        All | Any | True => true,
        None | False => false,
    })
}

fn interpret_matcher(s: &str, matcher: &Matcher) -> bool {
    use Matcher::*;
    match matcher {
        Equals(a) => s == a.0,
        Contains(a) => s.contains(&a.0),
        StartsWith(a) => s.starts_with(&a.0),
        EndsWith(a) => s.ends_with(&a.0),
        Matches(a) => a.0.is_match(s),
        Globs(a) => a.0.is_match(s),
    }
}

fn interpret_matcher_for_markers(markers: &[String], arg: &String) -> Option<bool> {
    Some(markers.contains(arg))
}

pub fn interpret_compound_selector(s: &CompoundSelector, c: &Context) -> Option<bool> {
    use CompoundSelectorName::*;
    Some(match s.name {
        File => interpret_matcher(c.file()?, &s.matcher),
        Name => interpret_matcher(&c.case()?.name, &s.matcher),
        NodeId => interpret_matcher(&c.case()?.node_id, &s.matcher),
        Package => interpret_matcher(&c.package, &s.matcher),
    })
}

fn interpret_not_expression(n: &NotExpression, c: &Context) -> Option<bool> {
    use NotExpression::*;
    match n {
        Not(n) => maybe_not(interpret_not_expression(n, c)),
        Simple(s) => interpret_simple_expression(s, c),
    }
}

fn interpret_and_expression(a: &AndExpression, c: &Context) -> Option<bool> {
    use AndExpression::*;
    match a {
        And(n, a) => maybe_and(
            interpret_not_expression(n, c),
            interpret_and_expression(a, c),
        ),
        Diff(n, a) => maybe_and(
            interpret_not_expression(n, c),
            maybe_not(interpret_and_expression(a, c)),
        ),
        Not(n) => interpret_not_expression(n, c),
    }
}

fn interpret_or_expression(o: &OrExpression, c: &Context) -> Option<bool> {
    use OrExpression::*;
    match o {
        Or(a, o) => maybe_or(
            interpret_and_expression(a, c),
            interpret_or_expression(o, c),
        ),
        And(a) => interpret_and_expression(a, c),
    }
}

pub fn interpret_simple_expression(s: &SimpleExpression, c: &Context) -> Option<bool> {
    use SimpleExpression::*;
    match s {
        Or(o) => interpret_or_expression(o, c),
        SimpleSelector(s) => interpret_simple_selector(s),
        CompoundSelector(s) => interpret_compound_selector(s, c),
        MarkersSelector(s) => interpret_matcher_for_markers(&c.case()?.markers, &s.contains.0),
    }
}

pub fn interpret_pattern(s: &Pattern, c: &Context) -> Option<bool> {
    interpret_or_expression(&s.0, c)
}

#[test]
fn simple_expression_simple_selector() {
    fn test_it(s: &str, file: Option<&str>, expected: Option<bool>) {
        let c = Context {
            package: "foo".into(),
            file: file.map(|f| f.into()),
            case: None,
        };
        let actual = interpret_simple_expression(&parse_str!(SimpleExpression, s).unwrap(), &c);
        assert_eq!(actual, expected);
    }

    // for all inputs, these expression evaluate as true
    for w in ["all", "any", "true"] {
        test_it(w, Some("foo.py"), Some(true));
        test_it(w, None, Some(true));
    }

    // for all inputs, these expression evaluate as false
    for w in ["none", "false"] {
        test_it(w, Some("foo.py"), Some(false));
        test_it(w, None, Some(false));
    }
}

#[cfg(test)]
fn test_compound_sel(s: &str, file: Option<&str>, expected: Option<bool>) {
    let c = Context {
        package: "foo".into(),
        file: file.map(|f| f.into()),
        case: None,
    };
    let actual = interpret_simple_expression(&parse_str!(SimpleExpression, s).unwrap(), &c);
    assert_eq!(actual, expected);
}

#[test]
fn simple_expression_compound_selector_starts_with() {
    let p = "file.starts_with(bar)";
    test_compound_sel(p, Some("barbaz"), Some(true));
    test_compound_sel(p, Some("bazbar"), Some(false));
    test_compound_sel(p, None, None);
}

#[test]
fn simple_expression_compound_selector_ends_with() {
    let p = "file.ends_with(bar)";
    test_compound_sel(p, Some("bazbar"), Some(true));
    test_compound_sel(p, Some("barbaz"), Some(false));
    test_compound_sel(p, None, None);
}

#[test]
fn simple_expression_compound_selector_equals() {
    let p = "file.equals(bar)";
    test_compound_sel(p, Some("bar"), Some(true));
    test_compound_sel(p, Some("baz"), Some(false));
    test_compound_sel(p, None, None);
}

#[test]
fn simple_expression_compound_selector_contains() {
    let p = "file.contains(bar)";
    test_compound_sel(p, Some("bazbarbin"), Some(true));
    test_compound_sel(p, Some("bazbin"), Some(false));
    test_compound_sel(p, None, None);
}

#[test]
fn simple_expression_compound_selector_matches() {
    let p = "file.matches(^[a-z]*$)";
    test_compound_sel(p, Some("bazbarbin"), Some(true));
    test_compound_sel(p, Some("baz-bin"), Some(false));
    test_compound_sel(p, None, None);
}

#[test]
fn simple_expression_compound_selector_globs() {
    let p = "file.globs(baz*)";
    test_compound_sel(p, Some("bazbarbin"), Some(true));
    test_compound_sel(p, Some("binbaz"), Some(false));
    test_compound_sel(p, None, None);
}

#[test]
fn markers_contains() {
    let c = Context {
        package: "foo".into(),
        file: Some("file.py".into()),
        case: Some(Case {
            name: "Test::case".into(),
            node_id: "file.py:Test::case".into(),
            markers: vec!["a".into(), "b".into(), "c".into()],
        }),
    };
    assert_eq!(
        interpret_simple_expression(
            &parse_str!(SimpleExpression, "markers.contains(a)").unwrap(),
            &c
        ),
        Some(true)
    );
    assert_eq!(
        interpret_simple_expression(
            &parse_str!(SimpleExpression, "markers.contains(b)").unwrap(),
            &c
        ),
        Some(true)
    );
    assert_eq!(
        interpret_simple_expression(
            &parse_str!(SimpleExpression, "markers.contains(c)").unwrap(),
            &c
        ),
        Some(true)
    );
    assert_eq!(
        interpret_simple_expression(
            &parse_str!(SimpleExpression, "markers.contains(d)").unwrap(),
            &c
        ),
        Some(false)
    );
}

#[cfg(test)]
fn test_compound_sel_case(
    s: &str,
    file: Option<&str>,
    package: impl Into<String>,
    case: impl Into<String>,
    node_id: impl Into<String>,
    expected: Option<bool>,
) {
    let c = Context {
        package: package.into(),
        file: file.map(|f| f.into()),
        case: Some(Case {
            name: case.into(),
            node_id: node_id.into(),
            markers: vec![],
        }),
    };
    let actual = interpret_simple_expression(&parse_str!(SimpleExpression, s).unwrap(), &c);
    assert_eq!(actual, expected);
}

#[test]
fn simple_expression_compound_selector_packge() {
    let p = "package.matches(^[a-z]*$)";
    test_compound_sel_case(
        p,
        Some("foo.py"),
        "bazbarbin",
        "test",
        "foo.py::Test::test",
        Some(true),
    );
    test_compound_sel_case(
        p,
        Some("foo.py"),
        "baz-bin",
        "test",
        "foo.py::Test::test",
        Some(false),
    );
    test_compound_sel_case(
        p,
        None,
        "baz-bin",
        "test",
        "foo.py::Test::test",
        Some(false),
    );
}

#[test]
fn simple_expression_compound_selector_name() {
    let p = "name.matches(^[a-z]*$)";
    test_compound_sel_case(
        p,
        Some("foo.py"),
        "pkg",
        "bazbarbin",
        "foo.py::Test::bazbarbin",
        Some(true),
    );
    test_compound_sel_case(
        p,
        Some("foo.py"),
        "pkg",
        "baz-bin",
        "foo.py::Test::baz-bin",
        Some(false),
    );
}

#[test]
fn and_or_not_diff_expressions() {
    fn test_it(s: &str, expected: bool) {
        let c = Context {
            package: "foo".into(),
            file: Some("foo_test.py".into()),
            case: Some(Case {
                name: "foo_test".into(),
                node_id: "foo_test.py::Test::foo_test".into(),
                markers: vec![],
            }),
        };
        let actual = interpret_pattern(&parse_str!(Pattern, s).unwrap(), &c);
        assert_eq!(actual, Some(expected));
    }

    test_it(
        "(package.equals(foo) || package.equals(bar)) && name.equals(foo_test)",
        true,
    );
    test_it("package.equals(foo) && name.equals(foo_test)", true);
    test_it("package.equals(foo) || name.equals(foo_test)", true);
    test_it("package.equals(foo) || name.equals(bar_test)", true);
    test_it("package.equals(foo) && !name.equals(bar_test)", true);
    test_it("package.equals(foo) - name.equals(bar_test)", true);

    test_it("package.equals(foo) && name.equals(bar_test)", false);
    test_it("package.equals(bar) || name.equals(bar_test)", false);
    test_it("package.equals(bar) || !name.equals(foo_test)", false);
    test_it("package.equals(foo) - name.equals(foo_test)", false);
}

#[test]
fn and_or_not_diff_maybe_expressions() {
    fn test_it(s: &str, expected: Option<bool>) {
        let c = Context {
            package: "foo".into(),
            file: Some("foo_test.py".into()),
            case: None,
        };
        let actual = interpret_pattern(&parse_str!(Pattern, s).unwrap(), &c);
        assert_eq!(actual, expected);
    }

    test_it(
        "(package.equals(foo) || package.equals(bar)) && name.equals(foo_test)",
        None,
    );
    test_it("package.equals(foo) && name.equals(foo_test)", None);
    test_it("name.equals(foo_test) && name.equals(bar_test)", None);
    test_it("name.equals(foo_test) && package.equals(foo)", None);
    test_it("package.equals(foo) && name.equals(bar_test)", None);
    test_it("package.equals(foo) && !name.equals(bar_test)", None);

    test_it("name.equals(foo_test) && package.equals(bar)", Some(false));
    test_it("package.equals(bar) && name.equals(foo_test)", Some(false));

    test_it("name.equals(foo_test) || name.equals(bar_test)", None);
    test_it("name.equals(foo_test) || package.equals(bar)", None);
    test_it("package.equals(bar) || name.equals(bar_test)", None);
    test_it("package.equals(bar) || !name.equals(foo_test)", None);

    test_it("name.equals(foo_test) || package.equals(foo)", Some(true));
    test_it("package.equals(foo) || name.equals(foo_test)", Some(true));
    test_it("package.equals(foo) || name.equals(bar_test)", Some(true));

    test_it("package.equals(foo) - name.equals(bar_test)", None);
    test_it("package.equals(foo) - name.equals(foo_test)", None);
}
