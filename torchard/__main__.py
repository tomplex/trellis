def main() -> None:
    from pathlib import Path

    from torchard.core.db import init_db, _DEFAULT_DB_PATH
    from torchard.core.manager import Manager
    from torchard.tui.app import TorchardApp

    import subprocess

    first_run = not Path(_DEFAULT_DB_PATH).exists()
    conn = init_db()
    manager = Manager(conn)
    if first_run:
        manager.scan_existing()
    result = TorchardApp(manager).run()

    # Handle tmux switching after the TUI is fully closed
    if isinstance(result, tuple):
        if result[0] == "session":
            subprocess.run(["tmux", "switch-client", "-t", result[1]])
        elif result[0] == "window":
            session_name, window_index = result[1], result[2]
            subprocess.run(["tmux", "select-window", "-t", f"{session_name}:{window_index}"])
            subprocess.run(["tmux", "switch-client", "-t", session_name])


if __name__ == "__main__":
    main()
