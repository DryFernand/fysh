use crate::Shell;

/// Convierte una línea en argumentos.
/// Soporta: espacios, "comillas dobles" (expanden $VAR), 'comillas simples' (literales).
pub fn parse(line: &str, shell: &Shell) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    // BOM: marca invisible que agregan PowerShell y Notepad al inicio del texto.
    let mut chars = line.trim_start_matches('\u{feff}').trim().chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    args.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    cur.push(c);
                }
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '$' => expand_var(&mut chars, &mut cur, shell),
                        _ => cur.push(c),
                    }
                }
            }
            '$' => {
                in_word = true;
                expand_var(&mut chars, &mut cur, shell);
            }
            _ => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        args.push(cur);
    }
    args
}

/// Lee el nombre tras '$' y agrega su valor. $? = código de salida del último comando.
fn expand_var(chars: &mut std::iter::Peekable<std::str::Chars>, out: &mut String, shell: &Shell) {
    if chars.peek() == Some(&'?') {
        chars.next();
        out.push_str(&shell.last_status.to_string());
        return;
    }
    let mut name = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_alphanumeric() || c == '_' {
            name.push(c);
            chars.next();
        } else {
            break;
        }
    }
    if name.is_empty() {
        out.push('$');
    } else {
        out.push_str(&std::env::var(&name).unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing() {
        let sh = Shell { last_status: 3, running: true };
        std::env::set_var("FYSH_T", "hola");
        assert_eq!(parse("  ls   -la  ", &sh), ["ls", "-la"]);
        assert_eq!(parse(r#"echo "a b" 'c $X'"#, &sh), ["echo", "a b", "c $X"]);
        assert_eq!(parse(r#"echo $FYSH_T "$FYSH_T!" $?"#, &sh), ["echo", "hola", "hola!", "3"]);
        assert_eq!(parse("echo ''", &sh), ["echo", ""]);
        assert_eq!(parse("\u{feff}echo hi", &sh), ["echo", "hi"]);
    }
}
