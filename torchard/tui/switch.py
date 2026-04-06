"""Shared switch-file mechanism for deferring tmux actions until after TUI exits."""

import json
import tempfile
from pathlib import Path

SWITCH_FILE = Path(tempfile.gettempdir()) / "torchard-switch.json"


def write_switch(action: dict) -> None:
    SWITCH_FILE.write_text(json.dumps(action))
