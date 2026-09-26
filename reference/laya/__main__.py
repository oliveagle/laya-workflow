"""python -m code.laya — Laya workflow engine entry point.

Usage:
    python -m code.laya                    # run all demos
    python -m code.laya --server           # start HTTP server on :8100
    python -m code.laya --server --port 9000
    python -m code.laya --compose          # run composition demos
    python -m code.laya --list             # list available workflows
"""
from __future__ import annotations
import sys


def main():
    args = sys.argv[1:]

    if "--server" in args:
        from code.laya.server import main as server_main
        sys.argv = [sys.argv[0]] + [a for a in args if a != "--server"]
        server_main()
    elif "--compose" in args:
        from code.laya.cli_compose import main as compose_main
        compose_main()
    elif "--list" in args:
        from code.laya.server import WORKFLOWS
        print("Available Laya workflows:")
        for name in WORKFLOWS:
            print(f"  {name}")
        print(f"\nRun: python -m code.laya --server")
        print(f"POST /run/<workflow_name> with {{\"state\": {{...}}}}")
    else:
        from code.laya.run_all import main as demo_main
        demo_main()


if __name__ == "__main__":
    main()
