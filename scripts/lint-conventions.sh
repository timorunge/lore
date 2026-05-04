#!/usr/bin/env bash
# lint-conventions.sh -- enforce code conventions defined in AGENTS.md
#
# Checks:
#   1.  No .unwrap() in production code (tests and const/static allowed)
#   2.  Import group ordering: crate:: must not appear before std::
#   3.  pub fn/struct/enum only in pub-mod-reachable files
#   4.  No `use super::` outside mod tests blocks
#   5.  `unsafe` requires `// SAFETY:` comment
#   6.  No `let _ = call()` (ignored errors)
#   7.  `mod tests` must be last item in file
#   8.  `#[default]` variant must be first in enum
#   9.  No `dbg!()` in production code
#  10.  No println!/eprintln! in library code
#  11.  Import groups separated by blank lines
#  12.  `deny_unknown_fields` on config Deserialize structs
#  13.  Architecture boundary -- forbidden cross-module imports
#  14.  No section-divider comment banners
#  15.  ASCII only in comments and markdown
#
# Exit codes: 0 = clean, 1 = violations found

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC_DIR="$REPO_ROOT/src"
CLI_SRC_DIR="$REPO_ROOT/cli/src"

violations=0

red()    { printf '\033[31m%s\033[0m\n' "$*"; }
green()  { printf '\033[32m%s\033[0m\n' "$*"; }

flag_violation() {
    red "VIOLATION [$1] $2"
    violations=$((violations + 1))
}

# Check 1: No .unwrap() in production code

check_unwrap() {
    local file="$1"
    local rel="${file#$REPO_ROOT/}"

    awk -v file="$rel" '
    BEGIN {
        in_test = 0; test_depth = 0; in_lazy = 0; lazy_depth = 0
        brace_depth = 0; pending_test = 0; pending_lazy = 0
        in_write = 0; in_pstyle = 0; found = 0
    }
    {
        line = $0; lineno = NR

        if (line ~ /#\[cfg\(test\)\]/) pending_test = 1
        if (line ~ /LazyLock::new/ || line ~ /OnceLock/ || line ~ /lazy_static!/ || line ~ /Lazy::new/)
            pending_lazy = 1

        # Track brace depth for cfg(test) and lazy-static blocks.
        # Limitation: counts braces inside string literals too, but matching
        # braces cancel out in practice.
        n = split(line, chars, "")
        for (i = 1; i <= n; i++) {
            c = chars[i]
            if (c == "{") {
                brace_depth++
                if (pending_test) {
                    if (in_test == 0) test_depth = brace_depth
                    in_test++
                    pending_test = 0
                }
                if (pending_lazy) {
                    if (in_lazy == 0) lazy_depth = brace_depth
                    in_lazy++
                    pending_lazy = 0
                }
            } else if (c == "}") {
                if (in_test > 0 && brace_depth == test_depth) in_test = 0
                if (in_lazy > 0 && brace_depth == lazy_depth) in_lazy = 0
                brace_depth--
            }
        }

        if (in_test > 0 || in_lazy > 0) next

        stripped = line
        sub(/^[[:space:]]*/, "", stripped)
        if (stripped ~ /^\/\//) next
        if (line ~ /^[[:space:]]*(pub[[:space:]]+)?(const|static)[[:space:]]/) next
        if (line ~ /LazyLock::new/ || line ~ /OnceLock::new/ || line ~ /Lazy::new/ || line ~ /lazy_static!/) next

        # write!/writeln! to String is infallible
        if (line ~ /write(ln)?!\s*\(/) { in_write = 1 }
        if (in_write && line ~ /\.unwrap\(\)/) { in_write = 0; next }
        if (in_write && line ~ /;/) { in_write = 0 }

        # ProgressStyle::with_template() with hardcoded templates is infallible
        if (line ~ /ProgressStyle::with_template/) { in_pstyle = 1 }
        if (in_pstyle && line ~ /\.unwrap\(\)/) { in_pstyle = 0; next }
        if (in_pstyle && line ~ /;/) { in_pstyle = 0 }

        # Regex::new with literal patterns is infallible
        if (line ~ /Regex::new\s*\(/ && line ~ /\.unwrap\(\)/) next
        if (line ~ /caps\.get\([0-9]\)/ && line ~ /\.unwrap\(\)/) next

        if (line ~ /\.unwrap\(\)/) {
            printf "  %s:%d: %s\n", file, lineno, line
            found++
        }
    }
    END { exit (found > 0) ? 1 : 0 }
    ' "$file"
}

echo "=== Check 1: No .unwrap() in production code ==="
unwrap_found=0

while IFS= read -r -d '' file; do
    case "$file" in
        */tests/* | *test_helpers.rs | *_test.rs) continue ;;
    esac
    if ! check_unwrap "$file"; then
        rel="${file#$REPO_ROOT/}"
        flag_violation "CONV-UNWRAP" "$rel contains .unwrap() outside test/const/static context (see lines above)"
        unwrap_found=1
    fi
done < <(find "$SRC_DIR" ${CLI_SRC_DIR:+"$CLI_SRC_DIR"} -name "*.rs" -print0)

if [ "$unwrap_found" -eq 0 ]; then
    green "  OK -- no bare .unwrap() in production code"
fi

# Check 2: Import group ordering (crate:: must come after std::)

check_import_order() {
    local file="$1"
    local rel="${file#$REPO_ROOT/}"

    awk -v file="$rel" '
    BEGIN { saw_crate = 0; found = 0 }
    /^use / {
        if ($0 ~ /^use (crate|super)::/) {
            saw_crate = 1
        } else if ($0 ~ /^use std::/ || $0 ~ /^use core::/) {
            if (saw_crate) {
                printf "  %s:%d: std:: import appears after crate:: import\n", file, NR
                found++
            }
        }
    }
    /^[^[:space:]\/\n#]/ && !/^use / && !/^\/\// && !/^$/ { saw_crate = 0 }
    END { exit (found > 0) ? 1 : 0 }
    ' "$file"
}

echo ""
echo "=== Check 2: Import group ordering (crate:: before std::) ==="
import_found=0

while IFS= read -r -d '' file; do
    if ! check_import_order "$file"; then
        rel="${file#$REPO_ROOT/}"
        flag_violation "CONV-IMPORTS" "$rel has std:: imports appearing after crate:: imports"
        import_found=1
    fi
done < <(find "$SRC_DIR" ${CLI_SRC_DIR:+"$CLI_SRC_DIR"} -name "*.rs" -print0)

if [ "$import_found" -eq 0 ]; then
    green "  OK -- no import ordering violations"
fi

# Check 3: pub fn/struct/enum only in pub-mod-reachable files
#
# Items with bare `pub` visibility (not pub(crate)/pub(super)/pub(in ...))
# must be in files reachable through `pub mod` from a lib.rs.  This catches
# items that are unnecessarily public.

echo ""
echo "=== Check 3: pub items only in pub-mod-reachable files ==="
pub_found=0
pub_reexported=0

pub_modules=""
if [ -f "$SRC_DIR/lib.rs" ]; then
    pub_modules=$(grep -E '^\s*pub\s+mod\s+' "$SRC_DIR/lib.rs" | sed 's/.*pub *mod *//;s/[; ].*//' | tr '\n' '|')
    pub_modules="${pub_modules%|}"
fi

cli_pub_modules=""
if [ -f "$CLI_SRC_DIR/lib.rs" ]; then
    cli_pub_modules=$(grep -E '^\s*pub\s+mod\s+' "$CLI_SRC_DIR/lib.rs" | sed 's/.*pub *mod *//;s/[; ].*//' | tr '\n' '|')
    cli_pub_modules="${cli_pub_modules%|}"
fi

while IFS= read -r -d '' file; do
    case "$file" in
        */lib.rs | */tests/*) continue ;;
    esac

    rel="${file#$REPO_ROOT/}"

    is_reexported=0
    if [ -n "$pub_modules" ]; then
        if echo "$rel" | grep -qE "^src/($pub_modules)/|^src/($pub_modules)\.rs$"; then
            is_reexported=1
        fi
    fi
    if [ "$is_reexported" -eq 0 ] && [ -n "$cli_pub_modules" ]; then
        if echo "$rel" | grep -qE "^cli/src/($cli_pub_modules)/|^cli/src/($cli_pub_modules)\.rs$"; then
            is_reexported=1
        fi
    fi

    while IFS= read -r match; do
        if [ "$is_reexported" -eq 1 ]; then
            pub_reexported=$((pub_reexported + 1))
        else
            flag_violation "CONV-PUB" "$match"
            pub_found=1
        fi
    done < <(
        grep -n "^\s*pub fn \|^\s*pub struct \|^\s*pub enum \|^\s*pub type " "$file" \
            | grep -v "pub(crate)\|pub(super)\|pub(in " \
            | sed "s|^|$rel:|"
    )
done < <(find "$SRC_DIR" ${CLI_SRC_DIR:+"$CLI_SRC_DIR"} -name "*.rs" -print0)

if [ "$pub_found" -eq 0 ] && [ "$pub_reexported" -eq 0 ]; then
    green "  OK -- no bare pub fn/struct/enum/type in non-lib.rs files"
elif [ "$pub_found" -eq 0 ]; then
    green "  OK -- no unexpected pub items ($pub_reexported in pub mod re-exported modules)"
fi

# Check 4: No `use super::` outside mod tests blocks

check_use_super() {
    local file="$1"
    local rel="${file#$REPO_ROOT/}"

    awk -v file="$rel" '
    BEGIN {
        in_tests = 0; tests_depth = 0; pending_tests = 0
        brace_depth = 0; found = 0
    }
    {
        line = $0
        if (line ~ /mod[[:space:]]+tests[[:space:]]*\{/ ||
            line ~ /mod[[:space:]]+tests[[:space:]]*$/)
            pending_tests = 1

        n = split(line, chars, "")
        for (i = 1; i <= n; i++) {
            c = chars[i]
            if (c == "{") {
                brace_depth++
                if (pending_tests) {
                    if (in_tests == 0) tests_depth = brace_depth
                    in_tests = 1
                    pending_tests = 0
                }
            } else if (c == "}") {
                if (in_tests && brace_depth == tests_depth) in_tests = 0
                brace_depth--
            }
        }

        if (!in_tests && line ~ /^[[:space:]]*use[[:space:]]+super::/) {
            printf "  %s:%d: %s\n", file, NR, line
            found++
        }
    }
    END { exit (found > 0) ? 1 : 0 }
    ' "$file"
}

echo ""
echo "=== Check 4: No \`use super::\` outside mod tests blocks ==="
super_found=0

while IFS= read -r -d '' file; do
    if ! check_use_super "$file"; then
        rel="${file#$REPO_ROOT/}"
        flag_violation "CONV-SUPER" "$rel has \`use super::\` outside a mod tests block (see lines above)"
        super_found=1
    fi
done < <(find "$SRC_DIR" ${CLI_SRC_DIR:+"$CLI_SRC_DIR"} -name "*.rs" -print0)

if [ "$super_found" -eq 0 ]; then
    green "  OK -- no bare \`use super::\` outside mod tests blocks"
fi

# Check 5: unsafe requires // SAFETY: comment

check_unsafe_safety() {
    local file="$1"
    local rel="${file#$REPO_ROOT/}"

    awk -v file="$rel" '
    BEGIN { found = 0; saw_safety = 0 }
    {
        stripped = $0
        sub(/^[[:space:]]*/, "", stripped)

        if (stripped ~ /^\/\//) {
            if (stripped ~ /SAFETY:/) saw_safety = 1
            next
        }

        is_unsafe = 0
        if (stripped ~ /^unsafe[[:space:]]/ ||
            stripped ~ /[^_a-zA-Z0-9"]unsafe[[:space:]]*\{/ ||
            stripped ~ /[^_a-zA-Z0-9"]unsafe[[:space:]]+fn[[:space:]]/)
            is_unsafe = 1

        if (is_unsafe && !saw_safety) {
            printf "  %s:%d: %s\n", file, NR, $0
            found++
        }

        if (stripped != "") saw_safety = 0
    }
    END { exit (found > 0) ? 1 : 0 }
    ' "$file"
}

echo ""
echo "=== Check 5: \`unsafe\` requires \`// SAFETY:\` comment ==="
unsafe_found=0

while IFS= read -r -d '' file; do
    case "$file" in */tests/*) continue ;; esac
    if ! check_unsafe_safety "$file"; then
        rel="${file#$REPO_ROOT/}"
        flag_violation "CONV-UNSAFE" "$rel has \`unsafe\` without a preceding \`// SAFETY:\` comment (see lines above)"
        unsafe_found=1
    fi
done < <(find "$SRC_DIR" ${CLI_SRC_DIR:+"$CLI_SRC_DIR"} -name "*.rs" -print0)

if [ "$unsafe_found" -eq 0 ]; then
    green "  OK -- all unsafe blocks have // SAFETY: comments"
fi

# Check 6: No let _ = call() (ignored errors)

check_ignored_errors() {
    local file="$1"
    local rel="${file#$REPO_ROOT/}"

    awk -v file="$rel" '
    BEGIN {
        in_test = 0; test_depth = 0; brace_depth = 0
        pending_test = 0; found = 0
    }
    {
        line = $0
        if (line ~ /#\[cfg\(test\)\]/) pending_test = 1

        n = split(line, chars, "")
        for (i = 1; i <= n; i++) {
            c = chars[i]
            if (c == "{") {
                brace_depth++
                if (pending_test) {
                    if (in_test == 0) test_depth = brace_depth
                    in_test++
                    pending_test = 0
                }
            } else if (c == "}") {
                if (in_test > 0 && brace_depth == test_depth) in_test = 0
                brace_depth--
            }
        }

        if (in_test > 0) next

        stripped = line
        sub(/^[[:space:]]*/, "", stripped)
        if (stripped ~ /^\/\//) next

        if (line ~ /let[[:space:]]+_[[:space:]]*=[[:space:]].*\(/) {
            printf "  %s:%d: %s\n", file, NR, line
            found++
        }
    }
    END { exit (found > 0) ? 1 : 0 }
    ' "$file"
}

echo ""
echo "=== Check 6: No \`let _ = call()\` -- ignored errors ==="
ignore_found=0

while IFS= read -r -d '' file; do
    case "$file" in
        */tests/* | *test_helpers.rs | *_test.rs) continue ;;
    esac
    if ! check_ignored_errors "$file"; then
        rel="${file#$REPO_ROOT/}"
        flag_violation "CONV-IGNORE" "$rel has \`let _ = ...()\` that may discard an error (see lines above)"
        ignore_found=1
    fi
done < <(find "$SRC_DIR" ${CLI_SRC_DIR:+"$CLI_SRC_DIR"} -name "*.rs" -print0)

if [ "$ignore_found" -eq 0 ]; then
    green "  OK -- no ignored errors found"
fi

# Check 7: mod tests must be last item in file

check_tests_at_bottom() {
    local file="$1"
    local rel="${file#$REPO_ROOT/}"

    awk -v file="$rel" '
    BEGIN {
        in_tests = 0; tests_depth = 0; pending_cfg_test = 0
        brace_depth = 0; tests_ended = 0; found = 0
    }
    {
        line = $0

        if (line ~ /#\[cfg\(test\)\]/) pending_cfg_test = 1

        if (pending_cfg_test && line ~ /mod[[:space:]]+tests[[:space:]]*[\{]?[[:space:]]*$/) {
            pending_cfg_test = 0
        } else if (pending_cfg_test) {
            stripped = line
            sub(/^[[:space:]]*/, "", stripped)
            if (stripped != "" && stripped !~ /^#\[/ && stripped !~ /^\/\//)
                pending_cfg_test = 0
        }

        n = split(line, chars, "")
        for (i = 1; i <= n; i++) {
            c = chars[i]
            if (c == "{") {
                brace_depth++
                if (line ~ /mod[[:space:]]+tests/ && in_tests == 0) {
                    tests_depth = brace_depth
                    in_tests = 1
                }
            } else if (c == "}") {
                if (in_tests && brace_depth == tests_depth) {
                    in_tests = 0
                    tests_ended = 1
                }
                brace_depth--
            }
        }

        if (tests_ended && !in_tests) {
            stripped = line
            sub(/^[[:space:]]*/, "", stripped)
            if (stripped != "" && stripped != "}") {
                printf "  %s:%d: %s\n", file, NR, line
                found++
            }
        }
    }
    END { exit (found > 0) ? 1 : 0 }
    ' "$file"
}

echo ""
echo "=== Check 7: \`mod tests\` must be last item in file ==="
tests_bottom_found=0

while IFS= read -r -d '' file; do
    if ! grep -q '#\[cfg(test)\]' "$file"; then
        continue
    fi
    if ! check_tests_at_bottom "$file"; then
        rel="${file#$REPO_ROOT/}"
        flag_violation "CONV-TESTS" "$rel has code after the \`mod tests\` block (see lines above)"
        tests_bottom_found=1
    fi
done < <(find "$SRC_DIR" ${CLI_SRC_DIR:+"$CLI_SRC_DIR"} -name "*.rs" -print0)

if [ "$tests_bottom_found" -eq 0 ]; then
    green "  OK -- all test modules are at the bottom of their files"
fi

# Check 8: #[default] variant must be first in enum

check_default_first() {
    local file="$1"
    local rel="${file#$REPO_ROOT/}"

    awk -v file="$rel" '
    BEGIN { in_enum = 0; saw_variant = 0; found = 0 }
    {
        stripped = $0
        sub(/^[[:space:]]*/, "", stripped)

        if (stripped ~ /^(pub([[:space:]]*\([^)]*\))?[[:space:]]+)?enum[[:space:]]/) {
            in_enum = 1; saw_variant = 0; next
        }
        if (!in_enum) next
        if (stripped ~ /^#\[/ && stripped !~ /^#\[default\]/) next
        if (stripped ~ /^\/\// || stripped == "") next
        if (stripped ~ /^\}/) { in_enum = 0; next }

        if (stripped == "#[default]") {
            if (saw_variant) {
                printf "  %s:%d: #[default] is not on the first variant\n", file, NR
                found++
            }
            next
        }

        if (stripped ~ /^[A-Z]/) saw_variant = 1
    }
    END { exit (found > 0) ? 1 : 0 }
    ' "$file"
}

echo ""
echo "=== Check 8: \`#[default]\` variant must be first in enum ==="
default_found=0

while IFS= read -r -d '' file; do
    if ! grep -q '#\[default\]' "$file"; then continue; fi
    if ! check_default_first "$file"; then
        rel="${file#$REPO_ROOT/}"
        flag_violation "CONV-ENUM" "$rel has \`#[default]\` on a non-first enum variant (see lines above)"
        default_found=1
    fi
done < <(find "$SRC_DIR" ${CLI_SRC_DIR:+"$CLI_SRC_DIR"} -name "*.rs" -print0)

if [ "$default_found" -eq 0 ]; then
    green "  OK -- all #[default] variants are first in their enums"
fi

# Check 9: No dbg!() in production code

echo ""
echo "=== Check 9: No \`dbg!()\` in production code ==="
dbg_found=0

while IFS= read -r -d '' file; do
    case "$file" in */tests/*) continue ;; esac
    matches=$(grep -n 'dbg!(' "$file" | grep -v '^\s*//' || true)
    if [ -n "$matches" ]; then
        rel="${file#$REPO_ROOT/}"
        echo "$matches" | while IFS= read -r match; do echo "  $rel:$match"; done
        flag_violation "CONV-DBG" "$rel contains \`dbg!()\` macro (see lines above)"
        dbg_found=1
    fi
done < <(find "$SRC_DIR" ${CLI_SRC_DIR:+"$CLI_SRC_DIR"} -name "*.rs" -print0)

if [ "$dbg_found" -eq 0 ]; then
    green "  OK -- no dbg!() found"
fi

# Check 10: No println!/eprintln! in library code
#
# Library crate (src/) should use tracing, not direct terminal output.
# Exception: signal handlers (platform.rs) where tracing is unsafe.

echo ""
echo "=== Check 10: No \`println!\`/\`eprintln!\` in library code ==="
print_found=0

lib_paths=()
for p in "$SRC_DIR/store" "$SRC_DIR/config" "$SRC_DIR/util" "$SRC_DIR/render" "$SRC_DIR/cache" "$SRC_DIR/net"; do
    [ -d "$p" ] && lib_paths+=("$p")
done
for p in "$SRC_DIR/types.rs" "$SRC_DIR/query.rs" "$SRC_DIR/llm.rs" "$SRC_DIR/lib.rs"; do
    [ -f "$p" ] && lib_paths+=("$p")
done

if [ "${#lib_paths[@]}" -gt 0 ]; then
    while IFS= read -r -d '' file; do
        rel="${file#$REPO_ROOT/}"
        # Signal handlers can't use tracing safely
        case "$rel" in src/util/platform.rs) continue ;; esac

        matches=$(grep -n 'println!\|eprintln!' "$file" \
            | grep -v '^\s*//' \
            | grep -v '//.*println!\|//.*eprintln!' \
            || true)
        if [ -n "$matches" ]; then
            echo "$matches" | while IFS= read -r match; do echo "  $rel:$match"; done
            flag_violation "CONV-PRINT" "$rel uses println!/eprintln! (library code should use tracing)"
            print_found=1
        fi
    done < <(find "${lib_paths[@]}" -name "*.rs" -print0)
fi

if [ "$print_found" -eq 0 ]; then
    green "  OK -- no println!/eprintln! in library code"
fi

# Check 11: Import group blank-line separation

check_import_separation() {
    local file="$1"
    local rel="${file#$REPO_ROOT/}"

    awk -v file="$rel" '
    BEGIN { current_group = 0; prev_blank = 0; found = 0 }
    /^use / {
        new_group = 0
        if ($0 ~ /^use std::/ || $0 ~ /^use core::/) new_group = 1
        else if ($0 ~ /^use (crate|super)::/) new_group = 3
        else new_group = 2

        if (current_group > 0 && new_group != current_group && !prev_blank) {
            printf "  %s:%d: missing blank line between import groups\n", file, NR
            found++
        }
        current_group = new_group; prev_blank = 0; next
    }
    /^[[:space:]]*$/ { prev_blank = 1; next }
    /^[^[:space:]]/ { current_group = 0 }
    { prev_blank = 0 }
    END { exit (found > 0) ? 1 : 0 }
    ' "$file"
}

echo ""
echo "=== Check 11: Import group blank-line separation ==="
sep_found=0

while IFS= read -r -d '' file; do
    if ! check_import_separation "$file"; then
        rel="${file#$REPO_ROOT/}"
        flag_violation "CONV-IMPORT-SEP" "$rel has import groups without blank-line separation (see lines above)"
        sep_found=1
    fi
done < <(find "$SRC_DIR" ${CLI_SRC_DIR:+"$CLI_SRC_DIR"} -name "*.rs" -print0)

if [ "$sep_found" -eq 0 ]; then
    green "  OK -- import groups are properly separated"
fi

# Check 12: deny_unknown_fields on config Deserialize structs

check_deny_unknown_fields() {
    local file="$1"
    local rel="${file#$REPO_ROOT/}"

    awk -v file="$rel" '
    BEGIN { found = 0; pending_derive = 0; attrs = "" }
    {
        stripped = $0
        sub(/^[[:space:]]*/, "", stripped)

        if (stripped ~ /^#\[/) {
            attrs = attrs " " stripped
            if (stripped ~ /Deserialize/) pending_derive = 1
            next
        }
        if (stripped == "" || stripped ~ /^\/\//) next

        if (pending_derive && stripped ~ /^(pub([[:space:]]*\([^)]*\))?[[:space:]]+)?struct[[:space:]]/) {
            if (attrs ~ /untagged/ || attrs ~ /flatten/) {
                # incompatible with deny_unknown_fields
            } else if (attrs !~ /deny_unknown_fields/) {
                name = stripped
                sub(/^(pub([[:space:]]*\([^)]*\))?[[:space:]]+)?struct[[:space:]]+/, "", name)
                sub(/[[:space:]<{(].*/, "", name)
                printf "  %s:%d: struct %s derives Deserialize without deny_unknown_fields\n", file, NR, name
                found++
            }
        }
        pending_derive = 0; attrs = ""
    }
    END { exit (found > 0) ? 1 : 0 }
    ' "$file"
}

echo ""
echo "=== Check 12: \`deny_unknown_fields\` on config Deserialize structs ==="
deny_found=0

while IFS= read -r -d '' file; do
    if ! grep -q 'Deserialize' "$file"; then continue; fi
    if ! check_deny_unknown_fields "$file"; then
        rel="${file#$REPO_ROOT/}"
        flag_violation "CONV-DENY" "$rel has Deserialize struct(s) without \`deny_unknown_fields\` (see lines above)"
        deny_found=1
    fi
done < <(find "$SRC_DIR/config" -name "*.rs" -print0 2>/dev/null)

if [ "$deny_found" -eq 0 ]; then
    green "  OK -- all config Deserialize structs have deny_unknown_fields"
fi

# Check 13: Architecture boundary -- forbidden cross-module imports
#
# Each rule: "module_dir:forbidden_import_module ..."
# Root-level files (lib.rs, types.rs, query.rs, etc.) have no restrictions.

echo ""
echo "=== Check 13: Architecture boundary -- forbidden cross-module imports ==="
arch_found=0

check_arch_boundary() {
    local module="$1"; shift
    local module_dir="$SRC_DIR/$module"
    [ -d "$module_dir" ] || return 0
    for banned in "$@"; do
        matches=$(grep -rn --include='*.rs' "crate::${banned}" "$module_dir" \
            | awk -F: '{
                content = substr($0, index($0, $2":") + length($2) + 1)
                gsub(/^[[:space:]]+/, "", content)
                if (content ~ /^\/\//) next
                print
            }' || true)
        if [ -n "$matches" ]; then
            while IFS= read -r match; do
                rel="${match#$REPO_ROOT/}"
                echo "  $rel"
            done <<< "$matches"
            flag_violation "CONV-ARCH" "src/$module/ imports crate::$banned (forbidden boundary crossing)"
            arch_found=1
        fi
    done
}

check_arch_boundary store   config ingest render
check_arch_boundary config  ingest
check_arch_boundary render  ingest
check_arch_boundary util    store config render ingest query cache
check_arch_boundary net     ingest query render

if [ "$arch_found" -eq 0 ]; then
    green "  OK -- no forbidden cross-module imports"
fi

# Check 14: No section-divider comment banners

echo ""
echo "=== Check 14: No section-divider comment banners ==="
banner_found=0

while IFS= read -r -d '' file; do
    matches=$(grep -nE '^\s*//\s*(={4,}|-{4,}|─{4,}|━{4,}|#{4,}|\*{4,})' "$file" || true)
    if [ -n "$matches" ]; then
        rel="${file#$REPO_ROOT/}"
        echo "$matches" | while IFS= read -r match; do echo "  $rel:$match"; done
        flag_violation "CONV-BANNER" "$rel has section-divider comment banner(s) (see lines above)"
        banner_found=1
    fi
done < <(find "$SRC_DIR" ${CLI_SRC_DIR:+"$CLI_SRC_DIR"} -name "*.rs" -print0)

if [ "$banner_found" -eq 0 ]; then
    green "  OK -- no section-divider comment banners"
fi

# Check 15: ASCII only in comments and markdown
#
# Rust: flag non-ASCII in comment lines (// and ///).
# String literals and code are allowed to contain non-ASCII (CJK test data,
# copyright regex patterns, Unicode escape sequences, etc.).
#
# Markdown: flag non-ASCII in docs/ and top-level .md files.
# Exemptions: table rows (| ...) and fenced code blocks may contain
# non-English data (query examples, code snippets, etc.).

echo ""
echo "=== Check 15: ASCII only in comments and markdown ==="
ascii_found=0

check_ascii_comments() {
    local file="$1"
    local rel="${file#$REPO_ROOT/}"

    # Extract comment lines (// and ///) with line numbers, then check for non-ASCII bytes.
    matches=$(awk '/^[[:space:]]*\/\// { printf "%d:%s\n", NR, $0 }' "$file" \
        | LC_ALL=C grep '[^ -~	]' \
        || true)
    if [ -n "$matches" ]; then
        echo "$matches" | while IFS= read -r match; do echo "  $rel:$match"; done
        return 1
    fi
    return 0
}

while IFS= read -r -d '' file; do
    if ! check_ascii_comments "$file"; then
        rel="${file#$REPO_ROOT/}"
        flag_violation "CONV-ASCII" "$rel has non-ASCII characters in comments (see lines above)"
        ascii_found=1
    fi
done < <(find "$SRC_DIR" ${CLI_SRC_DIR:+"$CLI_SRC_DIR"} -name "*.rs" -print0)

check_ascii_markdown() {
    local file="$1"
    local rel="${file#$REPO_ROOT/}"

    # Skip table rows (|) and fenced code blocks (``` ... ```) which may
    # contain non-English data (query examples, code snippets, etc.).
    matches=$(awk '
    /^```/ { in_fence = !in_fence; next }
    in_fence { next }
    /^[[:space:]]*\|/ { next }
    { printf "%d:%s\n", NR, $0 }
    ' "$file" | LC_ALL=C grep '[^ -~	]' || true)
    if [ -n "$matches" ]; then
        echo "$matches" | while IFS= read -r match; do echo "  $rel:$match"; done
        return 1
    fi
    return 0
}

for md_file in \
    "$REPO_ROOT/README.md" \
    "$REPO_ROOT/CHANGELOG.md" \
    "$REPO_ROOT/AGENTS.md"; do
    [ -f "$md_file" ] || continue
    if ! check_ascii_markdown "$md_file"; then
        rel="${md_file#$REPO_ROOT/}"
        flag_violation "CONV-ASCII" "$rel has non-ASCII characters (see lines above)"
        ascii_found=1
    fi
done

if [ -d "$REPO_ROOT/docs" ]; then
    while IFS= read -r -d '' md_file; do
        if ! check_ascii_markdown "$md_file"; then
            rel="${md_file#$REPO_ROOT/}"
            flag_violation "CONV-ASCII" "$rel has non-ASCII characters (see lines above)"
            ascii_found=1
        fi
    done < <(find "$REPO_ROOT/docs" -name "*.md" -print0)
fi

if [ "$ascii_found" -eq 0 ]; then
    green "  OK -- no non-ASCII in comments or documentation"
fi

# Summary

echo ""
echo "=== Summary ==="
echo "  Violations : $violations"

if [ "$violations" -gt 0 ]; then
    red "FAILED -- $violations convention violation(s) found"
    exit 1
else
    green "PASSED -- no violations"
    exit 0
fi
