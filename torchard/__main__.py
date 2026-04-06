def main() -> None:
    from pathlib import Path

    from torchard.core.db import init_db, _DEFAULT_DB_PATH
    from torchard.core.manager import Manager
    from torchard.tui.app import TorchardApp

    import json
    import subprocess
    import tempfile

    SWITCH_FILE = Path(tempfile.gettempdir()) / "torchard-switch.json"

    # Clean up any stale switch file
    SWITCH_FILE.unlink(missing_ok=True)

    first_run = not Path(_DEFAULT_DB_PATH).exists()
    conn = init_db()
    manager = Manager(conn)
    if first_run:
        manager.scan_existing()
    TorchardApp(manager).run()

    # Handle tmux switching after the TUI is fully closed
    if SWITCH_FILE.exists():
        try:
            action = json.loads(SWITCH_FILE.read_text())
            if action["type"] == "session":
                r = subprocess.run(["tmux", "switch-client", "-t", action["target"]], capture_output=True, text=True)
                if r.returncode != 0:
                    print(f"switch-client failed: {r.stderr.strip()}")
            elif action["type"] == "window":
                target = f"{action['session']}:{action['window']}"
                r = subprocess.run(["tmux", "switch-client", "-t", target], capture_output=True, text=True)
                if r.returncode != 0:
                    print(f"switch-client -t {target} failed: {r.stderr.strip()}")
                else:
                    print(f"switch-client -t {target} succeeded")
        finally:
            SWITCH_FILE.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
