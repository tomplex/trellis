def main() -> None:
    from pathlib import Path

    from torchard.core.db import init_db, _DEFAULT_DB_PATH
    from torchard.core.manager import Manager
    from torchard.tui.app import TorchardApp

    first_run = not Path(_DEFAULT_DB_PATH).exists()
    conn = init_db()
    manager = Manager(conn)
    if first_run:
        manager.scan_existing()
    TorchardApp(manager).run()


if __name__ == "__main__":
    main()
