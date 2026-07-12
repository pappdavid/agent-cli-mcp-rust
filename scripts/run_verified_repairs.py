#!/usr/bin/env python3
"""Normalize known pre-rustfmt shapes, then run the strict reviewed repair."""

from pathlib import Path

from apply_verified_repairs import main


def normalize_known_shapes() -> None:
    path = Path("src/tools.rs")
    text = path.read_text()
    compact = '        interactive_prompt_arg: if supports_interactive_prompt { Some("-i".to_string()) } else { Some("-i".to_string()) },'
    expanded = '''        interactive_prompt_arg: if supports_interactive_prompt {
            Some("-i".to_string())
        } else {
            Some("-i".to_string())
        },'''
    count = text.count(compact)
    if count != 1:
        raise RuntimeError(
            f"interactive capability compact shape: expected one match, found {count}"
        )
    path.write_text(text.replace(compact, expanded, 1))


if __name__ == "__main__":
    normalize_known_shapes()
    main()
