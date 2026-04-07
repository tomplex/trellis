"""Tests for classify_pane using real tmux captures."""

from pathlib import Path

from torchard.core.claude_session import classify_pane

_FIXTURES = Path(__file__).parent.parent / "claude-states"


def _load(name: str) -> str:
    return (_FIXTURES / name).read_text()


class TestClassifyPane:
    def test_thinking_is_working(self):
        # Spinner shows "✻ Envisioning…" — active thinking
        assert classify_pane(_load("claude-state-thinking.txt")) == "working"

    def test_needs_input_is_prompting(self):
        # Claude is showing the permission dialog
        assert classify_pane(_load("claude-state-needs-input.txt")) == "prompting"

    def test_done_thinking_is_idle(self):
        # Claude finished thinking, back at the prompt
        assert classify_pane(_load("claude-state-done-thinking.txt")) == "idle"

    def test_idle_with_checklist_is_idle(self):
        # Claude at the prompt with a task list visible
        assert classify_pane(_load("claude-state-idle-with-checklist.txt")) == "idle"

    def test_empty_is_idle(self):
        assert classify_pane("") == "idle"

    def test_bare_prompt(self):
        assert classify_pane("❯  \n") == "idle"

    def test_no_prompt_is_working(self):
        assert classify_pane("⏺ Bash(git status)\n  ⎿  Running…\n") == "working"
