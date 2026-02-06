# -*- coding: utf-8 -*-
"""
GeoEngine Client - CLI-based client library for GeoEngine.
Invokes the geoengine binary directly via subprocess (no HTTP proxy required).
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
        """
        Initialize the GeoEngine client.
        Locates the geoengine binary on the system.
        """
        self.binary = self._find_binary()

    @staticmethod
    def _find_binary() -> str:
        """Locate the geoengine binary."""
        path = shutil.which('geoengine')
        if path:
            return path

        # Fallback locations
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
        """
        Check the geoengine binary version.

        Returns:
            Dict with status and version information
        """
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

    def list_projects(self) -> List[Dict]:
        """
        List all registered projects.

        Returns:
            List of project summaries with name, path, and tools_count
        """
        result = subprocess.run(
            [self.binary, 'project', 'list', '--json'],
            capture_output=True, text=True, timeout=30
        )
        if result.returncode != 0:
            raise Exception(f"Failed to list projects: {result.stderr.strip()}")
        return json.loads(result.stdout)

    def get_project_tools(self, name: str) -> List[Dict]:
        """
        Get the list of tools available in a project.

        Args:
            name: Project name

        Returns:
            List of tool definitions with inputs and outputs
        """
        result = subprocess.run(
            [self.binary, 'project', 'tools', name],
            capture_output=True, text=True, timeout=30
        )
        if result.returncode != 0:
            raise Exception(f"Failed to get tools for '{name}': {result.stderr.strip()}")
        return json.loads(result.stdout)

    def run_tool(
        self,
        project: str,
        tool: str,
        inputs: Dict[str, Any],
        output_dir: Optional[str] = None,
        on_output: Optional[Callable[[str], None]] = None,
        is_cancelled: Optional[Callable[[], bool]] = None,
    ) -> Dict:
        """
        Run a GIS tool synchronously, streaming progress via on_output callback.

        Input parameters are passed as --input KEY=VALUE flags.
        The CLI maps these to script flags using the tool's input definitions
        (using map_to if specified, otherwise the input name).
        File paths are auto-mounted into the container.

        Args:
            project: Project name
            tool: Tool name to execute
            inputs: Input parameters as key-value pairs
            output_dir: Directory to write output files
            on_output: Callback called with each line of container output
            is_cancelled: Callback that returns True if the user requested cancellation

        Returns:
            Dict with status, exit_code, output_dir, and files list

        Raises:
            Exception: If the tool fails or is cancelled
        """
        cmd = [self.binary, 'project', 'run-tool', project, tool, '--json']
        if output_dir:
            cmd.extend(['--output-dir', output_dir])

        # Add input parameters as --input KEY=VALUE
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
            # Read stderr line-by-line for real-time progress
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

            # Read structured JSON result from stdout
            stdout_data = process.stdout.read()
            if process.returncode == 0 and stdout_data.strip():
                return json.loads(stdout_data)
            elif process.returncode != 0:
                # Try to parse JSON error from stdout
                if stdout_data.strip():
                    try:
                        return json.loads(stdout_data)
                    except json.JSONDecodeError:
                        pass
                raise Exception(f"Tool exited with code {process.returncode}")
            else:
                return {'status': 'completed', 'exit_code': 0, 'files': []}
        finally:
            if process.stdout:
                process.stdout.close()
            if process.stderr:
                process.stderr.close()


# Convenience function for standalone use
def run_tool(
    project: str,
    tool: str,
    inputs: Dict[str, Any],
    output_dir: Optional[str] = None,
    on_output: Optional[Callable[[str], None]] = None,
) -> Dict:
    """
    Run a GeoEngine tool and wait for completion.

    Args:
        project: Project name
        tool: Tool name
        inputs: Input parameters
        output_dir: Output directory
        on_output: Callback for progress output lines

    Returns:
        Dict with status, exit_code, output_dir, and files list
    """
    client = GeoEngineClient()
    return client.run_tool(project, tool, inputs, output_dir, on_output=on_output)


if __name__ == '__main__':
    import sys

    client = GeoEngineClient()

    try:
        info = client.version_check()
        print(f"GeoEngine: {info['version']}")

        projects = client.list_projects()
        print(f"\nRegistered Projects: {len(projects)}")
        for p in projects:
            print(f"  - {p['name']} ({p.get('tools_count', 0)} tools)")

    except Exception as e:
        print(f"Error: {e}")
        sys.exit(1)
