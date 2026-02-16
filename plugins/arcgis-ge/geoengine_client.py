# -*- coding: utf-8 -*-
"""
GeoEngine Client - CLI-based client library for GeoEngine.
Invokes the geoengine binary directly via subprocess.
Used by ArcGIS Pro toolbox and can be used standalone.
"""

import json
import os
import shutil
import subprocess
from typing import Any, Callable, Dict, List, Optional


class GeoEngineClient:
    """Client that invokes the geoengine CLI binary via subprocess."""

    def __init__(self):
        self.binary = self._find_binary()

    @staticmethod
    def _find_binary() -> str:
        """Locate the geoengine binary."""
        path = shutil.which('geoengine')
        if path:
            return path

        home = os.path.expanduser('~')
        for candidate in [
            os.path.join(home, '.geoengine', 'bin', 'geoengine'),
            os.path.join(home, '.cargo', 'bin', 'geoengine'),
        ]:
            if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
                return candidate

        raise FileNotFoundError(
            "geoengine binary not found. "
            "Install it from https://github.com/NikaGeospatial/geoengine "
            "or ensure it is on your PATH."
        )

    def version_check(self) -> Dict:
        """Check the geoengine binary version."""
        result = subprocess.run(
            [self.binary, '--version'],
            capture_output=True, text=True, timeout=10
        )
        if result.returncode != 0:
            raise Exception(f"geoengine version check failed: {result.stderr.strip()}")
        return {
            'status': 'healthy',
            'version': result.stdout.strip(),
        }

    def list_workers(self) -> List[Dict]:
        """List all registered workers."""
        result = subprocess.run(
            [self.binary, 'workers', '--json', '--gis', 'arcgis'],
            capture_output=True, text=True, timeout=30
        )
        if result.returncode != 0:
            raise Exception(f"Failed to list workers: {result.stderr.strip()}")
        return json.loads(result.stdout)

    def get_worker_tool(self, name: str) -> Optional[Dict]:
        """Get the tool/input description for a worker via `geoengine run <name> --describe`."""
        result = subprocess.run(
            [self.binary, 'describe', name, '--json'],
            capture_output=True, text=True, timeout=30
        )
        if result.returncode != 0:
            raise Exception(f"Failed to describe worker '{name}': {result.stderr.strip()}")
        return json.loads(result.stdout)

    def run_tool(
        self,
        worker: str,
        inputs: Dict[str, Any],
        on_output: Optional[Callable[[str], None]] = None,
        is_cancelled: Optional[Callable[[], bool]] = None,
    ) -> Dict:
        """Run a worker synchronously, streaming progress via on_output callback.

        Input parameters are passed as --input KEY=VALUE flags.
        File paths are auto-mounted into the container.

        Args:
            worker: Worker name
            inputs: Input parameters as key-value pairs
            on_output: Callback called with each line of container output
            is_cancelled: Callback that returns True if the user requested cancellation

        Returns:
            Dict with status, exit_code, and files list
        """
        cmd = [self.binary, 'run', worker, '--json']

        for key, value in inputs.items():
            if value is not None:
                cmd.extend(['--input', f'{key}={value}'])

        process = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

        try:
            for line in iter(process.stderr.readline, ''):
                if line:
                    stripped = line.rstrip('\n')
                    if on_output and stripped:
                        on_output(stripped)
                if is_cancelled and is_cancelled():
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
                    raise Exception("Job cancelled by user")

            process.wait()

            stdout_data = process.stdout.read()
            if process.returncode == 0 and stdout_data.strip():
                return json.loads(stdout_data)
            elif process.returncode != 0:
                error_detail: Any = stdout_data.strip() or f"exit code {process.returncode}"
                if stdout_data.strip():
                    try:
                        parsed = json.loads(stdout_data)
                        if isinstance(parsed, dict):
                            error_detail = (
                                parsed.get("error")
                                or parsed.get("detail")
                                or parsed
                            )
                        else:
                            error_detail = parsed
                    except json.JSONDecodeError:
                        error_detail = stdout_data.strip()
                raise Exception(f"GeoEngine tool failed: {error_detail}")
            else:
                return {'status': 'completed', 'exit_code': 0, 'files': []}
        finally:
            if process.stdout:
                process.stdout.close()
            if process.stderr:
                process.stderr.close()


# Convenience function for standalone use
def run_tool(
    worker: str,
    inputs: Dict[str, Any],
    on_output: Optional[Callable[[str], None]] = None,
) -> Dict:
    """Run a GeoEngine worker and wait for completion."""
    client = GeoEngineClient()
    return client.run_tool(worker, inputs, on_output=on_output)


if __name__ == '__main__':
    import sys

    client = GeoEngineClient()

    try:
        info = client.version_check()
        print(f"GeoEngine: {info['version']}")

        workers = client.list_workers()
        print(f"\nRegistered Workers: {len(workers)}")
        for w in workers:
            has_tool = "yes" if w.get('has_tool', False) else "no"
            print(f"  - {w['name']} (tool: {has_tool})")

    except Exception as e:
        print(f"Error: {e}")
        sys.exit(1)
