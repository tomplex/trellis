"""Tests for torchard.core.tmux — all subprocess calls are mocked."""

from __future__ import annotations

from unittest.mock import MagicMock, call, patch

import pytest

from torchard.core.tmux import (
    TmuxError,
    kill_session,
    list_sessions,
    new_session,
    new_window,
    session_exists,
    switch_client,
)


def _completed(returncode: int = 0, stdout: str = "", stderr: str = "") -> MagicMock:
    m = MagicMock()
    m.returncode = returncode
    m.stdout = stdout
    m.stderr = stderr
    return m


class TestListSessions:
    def test_returns_parsed_sessions(self):
        output = "work\t3\t1\nplay\t1\t0\n"
        with patch("subprocess.run", return_value=_completed(stdout=output)) as mock_run:
            result = list_sessions()

        mock_run.assert_called_once_with(
            ["tmux", "list-sessions", "-F", "#{session_name}\t#{session_windows}\t#{session_attached}"],
            capture_output=True,
            text=True,
            check=False,
        )
        assert result == [
            {"name": "work", "windows": 3, "attached": True},
            {"name": "play", "windows": 1, "attached": False},
        ]

    def test_returns_empty_list_on_error(self):
        with patch("subprocess.run", return_value=_completed(returncode=1)):
            result = list_sessions()
        assert result == []

    def test_returns_empty_list_when_no_output(self):
        with patch("subprocess.run", return_value=_completed(stdout="")):
            result = list_sessions()
        assert result == []


class TestSessionExists:
    def test_returns_true_when_session_found(self):
        with patch("subprocess.run", return_value=_completed(returncode=0)) as mock_run:
            assert session_exists("mysession") is True
        mock_run.assert_called_once_with(
            ["tmux", "has-session", "-t", "mysession"],
            capture_output=True, text=True, check=False,
        )

    def test_returns_false_when_session_missing(self):
        with patch("subprocess.run", return_value=_completed(returncode=1)):
            assert session_exists("nosession") is False


class TestNewSession:
    def test_creates_session(self):
        responses = [
            _completed(returncode=1),  # has-session -> doesn't exist
            _completed(returncode=0),  # new-session -> success
        ]
        with patch("subprocess.run", side_effect=responses) as mock_run:
            new_session("myapp", "/home/user/myapp")

        assert mock_run.call_count == 2
        mock_run.assert_called_with(
            ["tmux", "new-session", "-d", "-s", "myapp", "-c", "/home/user/myapp"],
            capture_output=True, text=True, check=False,
        )

    def test_raises_if_session_already_exists(self):
        with patch("subprocess.run", return_value=_completed(returncode=0)):
            with pytest.raises(TmuxError, match="already exists"):
                new_session("existing", "/tmp")

    def test_raises_on_creation_failure(self):
        responses = [
            _completed(returncode=1),   # has-session -> doesn't exist
            _completed(returncode=1, stderr="some tmux error"),  # new-session -> fail
        ]
        with patch("subprocess.run", side_effect=responses):
            with pytest.raises(TmuxError, match="Failed to create session"):
                new_session("bad", "/tmp")


class TestSwitchClient:
    def test_switches_to_session(self):
        with patch("subprocess.run", return_value=_completed()) as mock_run:
            switch_client("myapp")
        mock_run.assert_called_once_with(
            ["tmux", "switch-client", "-t", "myapp"],
            capture_output=True, text=True, check=False,
        )

    def test_raises_on_failure(self):
        with patch("subprocess.run", return_value=_completed(returncode=1, stderr="no client")):
            with pytest.raises(TmuxError, match="Failed to switch"):
                switch_client("ghost")


class TestNewWindow:
    def test_creates_window(self):
        with patch("subprocess.run", return_value=_completed()) as mock_run:
            new_window("myapp", "tests", "/home/user/myapp")
        mock_run.assert_called_once_with(
            ["tmux", "new-window", "-t", "myapp", "-n", "tests", "-c", "/home/user/myapp"],
            capture_output=True, text=True, check=False,
        )

    def test_raises_on_failure(self):
        with patch("subprocess.run", return_value=_completed(returncode=1, stderr="bad session")):
            with pytest.raises(TmuxError, match="Failed to create window"):
                new_window("noapp", "editor", "/tmp")


class TestKillSession:
    def test_kills_session(self):
        with patch("subprocess.run", return_value=_completed()) as mock_run:
            kill_session("myapp")
        mock_run.assert_called_once_with(
            ["tmux", "kill-session", "-t", "myapp"],
            capture_output=True, text=True, check=False,
        )

    def test_raises_on_failure(self):
        with patch("subprocess.run", return_value=_completed(returncode=1, stderr="no session")):
            with pytest.raises(TmuxError, match="Failed to kill session"):
                kill_session("ghost")
