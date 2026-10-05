use serde::{Deserialize, Serialize};

pub const MAX_SKILL_NAME_CHARS: usize = 96;
pub const MAX_SKILL_DESCRIPTION_CHARS: usize = 512;
pub const MAX_SKILL_DEFINITION_BYTES: usize = 64 * 1024;
pub const MAX_SKILL_FRONTMATTER_LINES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
}

/// Parse the canonical bounded Agent Skill frontmatter used by both Control
/// project discovery and the Runner operator-store installer. Only explicit
/// top-level `name` and `description` metadata are accepted; bodies are never
/// searched for inferred metadata.
///
/// Accepted scalar forms are the ones Skill authors use in practice: plain,
/// single-quoted (`''` escape), double-quoted (common backslash escapes),
/// literal/folded block scalars (`|`, `>`, with optional chomping and
/// indentation indicators) and indented plain continuation lines. Whitespace
/// in the result is collapsed to single spaces. A description longer than
/// `MAX_SKILL_DESCRIPTION_CHARS` is truncated with an ellipsis instead of
/// rejecting the Skill; names stay strict.
pub fn parse_skill_metadata(text: &str) -> Result<SkillMetadata, &'static str> {
    if text.len() > MAX_SKILL_DEFINITION_BYTES {
        return Err("skill_definition_too_large");
    }
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Err("skill_frontmatter_missing");
    }
    let frontmatter = lines
        .take(MAX_SKILL_FRONTMATTER_LINES)
        .collect::<Vec<_>>();
    let Some(close) = frontmatter.iter().position(|line| line.trim() == "---") else {
        return Err("skill_frontmatter_unclosed");
    };
    let frontmatter = &frontmatter[..close];
    let mut name = None::<String>;
    let mut description = None::<String>;
    let mut index = 0;
    while index < frontmatter.len() {
        let line = frontmatter[index];
        index += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || starts_indented(line) {
            continue;
        }
        let Some((key, raw_value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if !matches!(key, "name" | "description") {
            continue;
        }
        let continuation_start = index;
        while index < frontmatter.len()
            && (frontmatter[index].trim().is_empty() || starts_indented(frontmatter[index]))
        {
            index += 1;
        }
        let value =
            parse_frontmatter_scalar(raw_value.trim(), &frontmatter[continuation_start..index])?;
        match key {
            "name" if name.is_none() => name = Some(value),
            "description" if description.is_none() => description = Some(value),
            _ => return Err("skill_frontmatter_duplicate_field"),
        }
    }
    let name = name.ok_or("skill_name_missing")?;
    let description = description.ok_or("skill_description_missing")?;
    if name.is_empty()
        || name.chars().count() > MAX_SKILL_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err("skill_name_invalid");
    }
    if description.is_empty() || description.chars().any(char::is_control) {
        return Err("skill_description_invalid");
    }
    Ok(SkillMetadata {
        name,
        description: truncate_description(description),
    })
}

fn starts_indented(line: &str) -> bool {
    line.starts_with([' ', '\t'])
}

fn parse_frontmatter_scalar(raw: &str, continuation: &[&str]) -> Result<String, &'static str> {
    if raw.is_empty() && continuation.iter().all(|line| line.trim().is_empty()) {
        return Err("skill_frontmatter_scalar_invalid");
    }
    if let Some(header) = raw.strip_prefix(['|', '>']) {
        let header = header.split(" #").next().unwrap_or(header).trim();
        if header.len() > 2
            || !header
                .chars()
                .all(|indicator| matches!(indicator, '-' | '+' | '1'..='9'))
        {
            return Err("skill_frontmatter_scalar_invalid");
        }
        return Ok(collapse_whitespace(continuation.iter().copied()));
    }
    if matches!(
        raw.as_bytes().first(),
        Some(b'[' | b'{' | b'&' | b'*' | b'!' | b'|' | b'>')
    ) {
        return Err("skill_frontmatter_scalar_invalid");
    }
    if raw.starts_with('"') || raw.starts_with('\'') {
        let joined = collapse_whitespace(std::iter::once(raw).chain(continuation.iter().copied()));
        let unquoted = if raw.starts_with('"') {
            parse_double_quoted(&joined)?
        } else {
            parse_single_quoted(&joined)?
        };
        return Ok(collapse_whitespace(std::iter::once(unquoted.as_str())));
    }
    let first = raw.split(" #").next().unwrap_or(raw);
    Ok(collapse_whitespace(
        std::iter::once(first).chain(continuation.iter().copied()),
    ))
}

fn parse_double_quoted(raw: &str) -> Result<String, &'static str> {
    let inner = raw
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or("skill_frontmatter_scalar_invalid")?;
    let mut value = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Err("skill_frontmatter_scalar_invalid"),
            '\\' => match chars.next() {
                Some('"') => value.push('"'),
                Some('\\') => value.push('\\'),
                Some('/') => value.push('/'),
                Some('n' | 't' | 'r' | ' ') => value.push(' '),
                Some('u') => {
                    let hex = chars.by_ref().take(4).collect::<String>();
                    let decoded = (hex.len() == 4)
                        .then(|| u32::from_str_radix(&hex, 16).ok())
                        .flatten()
                        .and_then(char::from_u32)
                        .ok_or("skill_frontmatter_scalar_invalid")?;
                    value.push(decoded);
                }
                _ => return Err("skill_frontmatter_scalar_invalid"),
            },
            other => value.push(other),
        }
    }
    Ok(value)
}

fn parse_single_quoted(raw: &str) -> Result<String, &'static str> {
    let inner = raw
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .ok_or("skill_frontmatter_scalar_invalid")?;
    let mut value = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\'' {
            if chars.next() != Some('\'') {
                return Err("skill_frontmatter_scalar_invalid");
            }
        }
        value.push(ch);
    }
    Ok(value)
}

fn collapse_whitespace<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let mut value = String::new();
    for word in parts.flat_map(str::split_whitespace) {
        if !value.is_empty() {
            value.push(' ');
        }
        value.push_str(word);
    }
    value
}

fn truncate_description(description: String) -> String {
    if description.chars().count() <= MAX_SKILL_DESCRIPTION_CHARS {
        return description;
    }
    let mut truncated = description
        .chars()
        .take(MAX_SKILL_DESCRIPTION_CHARS - 1)
        .collect::<String>();
    truncated.truncate(truncated.trim_end().len());
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_parser_requires_explicit_simple_metadata() {
        let parsed = parse_skill_metadata(
            "---\nname: demo\ndescription: 'Use demo safely'\nlicense: MIT\n---\nPRIVATE_BODY",
        )
        .unwrap();
        assert_eq!(parsed.name, "demo");
        assert_eq!(parsed.description, "Use demo safely");
        for invalid in [
            "# no frontmatter\nname: guessed",
            "---\ndescription: only desc\n---\nname in body",
            "---\nname: x\n---\nbody description",
            "---\nname: x\ndescription: [a, b]\n---",
            "---\nname: x\ndescription: \"unterminated\n---",
            "---\nname: x\ndescription: \"bad \\q escape\"\n---",
            "---\nname: x\ndescription: |\n---",
            "---\nname: x\nname: y\ndescription: d\n---",
        ] {
            assert!(parse_skill_metadata(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn block_scalars_are_folded_to_one_line() {
        for indicator in [">", "|", ">-", "|+", "|2"] {
            let parsed = parse_skill_metadata(&format!(
                "---\nname: demo\ndescription: {indicator}\n  First line\n  second line.\n\n  Third.\nlicense: MIT\n---\nBODY"
            ))
            .unwrap();
            assert_eq!(parsed.description, "First line second line. Third.", "{indicator}");
        }
    }

    #[test]
    fn quoted_scalars_support_escapes_and_continuations() {
        let parsed = parse_skill_metadata(
            "---\nname: demo\ndescription: \"Mentions \\\"deck,\\\" a\\\\b and \\u00e9\"\n---",
        )
        .unwrap();
        assert_eq!(parsed.description, "Mentions \"deck,\" a\\b and é");
        let parsed =
            parse_skill_metadata("---\nname: 'it''s'\ndescription: plain start\n  continued here\n---")
                .unwrap();
        assert_eq!(parsed.name, "it's");
        assert_eq!(parsed.description, "plain start continued here");
    }

    #[test]
    fn nested_keys_do_not_shadow_top_level_metadata() {
        let parsed = parse_skill_metadata(
            "---\nmetadata:\n  name: nested\n  description: nested\nname: demo\ndescription: top\n---",
        )
        .unwrap();
        assert_eq!(parsed.name, "demo");
        assert_eq!(parsed.description, "top");
    }

    #[test]
    fn long_descriptions_are_truncated_and_long_names_rejected() {
        let long = "word ".repeat(200);
        let parsed =
            parse_skill_metadata(&format!("---\nname: demo\ndescription: {long}\n---")).unwrap();
        assert_eq!(parsed.description.chars().count() <= MAX_SKILL_DESCRIPTION_CHARS, true);
        assert!(parsed.description.ends_with('…'));
        let name = "n".repeat(MAX_SKILL_NAME_CHARS + 1);
        assert_eq!(
            parse_skill_metadata(&format!("---\nname: {name}\ndescription: d\n---")),
            Err("skill_name_invalid")
        );
    }
}
