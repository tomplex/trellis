"""Shared utilities for the TUI layer."""

from __future__ import annotations

import re


def safe_id(text: str) -> str:
    """Sanitize a string for use as a textual widget ID."""
    return re.sub(r"[^a-zA-Z0-9_-]", "_", text)


def truncate_end(text: str, max_len: int) -> str:
    """Truncate *text* from the right, appending '…' if it exceeds *max_len*."""
    if len(text) <= max_len:
        return text
    return text[: max_len - 1] + "…"


def truncate_start(text: str, max_len: int) -> str:
    """Truncate *text* from the left, prepending '…' if it exceeds *max_len*."""
    if len(text) <= max_len:
        return text
    return "…" + text[-(max_len - 1):]
