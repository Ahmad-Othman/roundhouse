//! String#bytes materializes the receiver once as unsigned byte integers.
use crate::expr::{Expr, ExprNode};
use crate::ty::Ty;

/// Targets whose string representation needs a byte-array bridge.
#[derive(Clone, Copy)]
pub enum Target {
    Rust,
    TypeScript,
    Crystal,
    Python,
    Kotlin,
    Swift,
    CSharp,
    Go,
    Elixir,
}

impl Target {
    fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Crystal => "crystal",
            Self::Python => "python",
            Self::Kotlin => "kotlin",
            Self::Swift => "swift",
            Self::CSharp => "csharp",
            Self::Go => "go",
            Self::Elixir => "elixir",
        }
    }
}

/// Classify the complete call before a backend separates its block from its
/// receiver. Only the no-argument, no-block form materializes an array; a
/// supplied block must never disappear into that bridge. Ruby and Spinel use
/// their native implementation, including its block semantics.
pub fn emit(
    e: &Expr,
    target: Target,
    emit_receiver: impl FnOnce(&Expr) -> String,
) -> Option<String> {
    if e.diagnostic.is_some() {
        return None;
    }
    let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        block,
        ..
    } = &*e.node
    else {
        return None;
    };
    if method.as_str() != "bytes" || recv.ty.as_ref() != Some(&Ty::Str) {
        return None;
    }
    let unsupported = if !args.is_empty() {
        Some("String#bytes accepts no positional arguments")
    } else if block.is_some() {
        Some("String#bytes with a block is not implemented for this target")
    } else {
        None
    };
    Some(match unsupported {
        Some(detail) => crate::emit::diagnostics::report_unsupported(
            e.span,
            target.name(),
            "String#bytes",
            detail,
        ),
        None => render(target, &emit_receiver(recv)),
    })
}

/// Preserve one receiver evaluation and one encoding pass. Unicode-native
/// strings expose their UTF-8 representation; Ruby/Spinel retain raw bytes.
pub fn render(target: Target, recv: &str) -> String {
    match target {
        Target::Rust => {
            format!("({recv}).as_bytes().iter().map(|byte| i64::from(*byte)).collect::<Vec<i64>>()")
        }
        Target::TypeScript => format!("Array.from(new TextEncoder().encode({recv}))"),
        Target::Crystal => format!("({recv}).bytes.map {{ |byte| byte.to_i64 }}"),
        Target::Python => format!("list(({recv}).encode(\"utf-8\"))"),
        Target::Kotlin => format!(
            "({recv}).toByteArray(Charsets.UTF_8).map {{ byte -> (byte.toInt() and 255).toLong() }}.toMutableList()"
        ),
        Target::Swift => format!("({recv}).utf8.map {{ Int($0) }}"),
        Target::CSharp => {
            format!("System.Text.Encoding.UTF8.GetBytes({recv}).Select(b => (long)b).ToList()")
        }
        Target::Go => format!(
            "func(text string) []int64 {{ out := make([]int64, len(text)); for i := 0; i < len(text); i++ {{ out[i] = int64(text[i]) }}; return out }}({recv})"
        ),
        Target::Elixir => format!(":binary.bin_to_list({recv})"),
    }
}
