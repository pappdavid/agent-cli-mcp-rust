#!/usr/bin/env python3
"""Normalize known pre-rustfmt shapes, then run the strict reviewed repair."""

from pathlib import Path

import apply_verified_repairs as repairs


def normalize_known_shapes() -> None:
    path = Path("src/tools.rs")
    text = path.read_text()

    compact_interactive = '        interactive_prompt_arg: if supports_interactive_prompt { Some("-i".to_string()) } else { Some("-i".to_string()) },'
    expanded_interactive = '''        interactive_prompt_arg: if supports_interactive_prompt {
            Some("-i".to_string())
        } else {
            Some("-i".to_string())
        },'''
    if text.count(compact_interactive) != 1:
        raise RuntimeError(
            "interactive capability compact shape did not match exactly once"
        )
    text = text.replace(compact_interactive, expanded_interactive, 1)

    if text.count("    if matching_run.is_some() {") != 1:
        raise RuntimeError("Jules matching-run condition did not match exactly once")
    if text.count("&matching_run.unwrap().id") != 1:
        raise RuntimeError("Jules matching-run unwrap did not match exactly once")
    text = text.replace(
        "    if matching_run.is_some() {",
        "    if let Some(run) = matching_run.as_ref() {",
        1,
    )
    text = text.replace("&matching_run.unwrap().id", "&run.id", 1)
    path.write_text(text)


def allow_preapplied_jules_fix() -> None:
    original = repairs.replace_once

    def guarded_replace(text: str, old: str, new: str, label: str) -> str:
        if label == "Jules matching-run unwrap" and text.count(old) == 0:
            return text
        return original(text, old, new, label)

    repairs.replace_once = guarded_replace


if __name__ == "__main__":
    normalize_known_shapes()
    allow_preapplied_jules_fix()
    repairs.main()
