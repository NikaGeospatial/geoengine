# -*- coding: utf-8 -*-
"""
GeoEngine QGIS Processing Provider
Invokes the geoengine CLI directly to provide containerized geoprocessing tools
as QGIS Processing algorithms.
"""

import json
import os
import shutil
import subprocess
from typing import Any, Callable, Dict, List, Optional

from qgis.core import (
    QgsProcessingAlgorithm,
    QgsProcessingContext,
    QgsProcessingFeedback,
    QgsProcessingParameterString,
    QgsProcessingParameterNumber,
    QgsProcessingParameterBoolean,
    QgsProcessingParameterFile,
    QgsProcessingParameterEnum,
    QgsProcessingProvider,
)


# ---------------------------------------------------------------------------
# CLI Client
# ---------------------------------------------------------------------------

class GeoEngineCLIClient:
    """Client that invokes the geoengine CLI binary via subprocess."""

    def __init__(self):
        self.binary = self._find_binary()

    @staticmethod
    def _find_binary() -> str:
        """Locate the geoengine binary."""
        current_path = os.environ.get("PATH", "")
        search_path = current_path
        if "/usr/local/bin" not in current_path:
            search_path = current_path + ":/usr/local/bin"
        path = shutil.which('geoengine', path=search_path)
        if path:
            if search_path != current_path:
                os.environ["PATH"] = search_path
            return path

        home = os.path.expanduser('~')
        for candidate in [
            os.path.join(home, '.geoengine', 'bin', 'geoengine'),
            os.path.join(home, '.cargo', 'bin', 'geoengine')
        ]:
            if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
                return candidate

        raise FileNotFoundError(
            "geoengine binary not found. "
            "Install it from https://github.com/NikaGeospatial/geoengine "
            "or ensure it is on your PATH."
        )

    def version_check(self) -> Dict:
        result = subprocess.run(
            [self.binary, '--version'],
            capture_output=True, text=True, timeout=10
        )
        if result.returncode != 0:
            raise Exception(f"geoengine version check failed: {result.stderr.strip()}")
        return {'status': 'healthy', 'version': result.stdout.strip()}

    def list_workers(self) -> List[Dict]:
        result = subprocess.run(
            [self.binary, 'workers', '--json', '--gis', 'qgis'],
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
        """Run a worker synchronously, streaming container output via on_output.

        Input parameters are passed as --input KEY=VALUE flags.
        File paths are auto-mounted into the container.
        """
        cmd = [self.binary, 'run', worker, '--json']

        for key, value in inputs.items():
            if value is not None:
                cmd.extend(['--input', f'{key}={value}'])

        process = subprocess.Popen(
            cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
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
                error_detail = {}
                if stdout_data.strip():
                    try:
                        error_detail = json.loads(stdout_data)
                    except json.JSONDecodeError:
                        pass
                raise Exception(
                    f"Tool exited with code {process.returncode}: "
                    f"{error_detail.get('error', 'unknown error')}"
                )
            else:
                return {'status': 'completed', 'exit_code': 0, 'files': []}
        finally:
            if process.stdout:
                process.stdout.close()
            if process.stderr:
                process.stderr.close()


# ---------------------------------------------------------------------------
# QGIS Processing Provider
# ---------------------------------------------------------------------------

class GeoEngineProvider(QgsProcessingProvider):
    """QGIS Processing provider for GeoEngine tools."""

    def __init__(self):
        super().__init__()
        self._algorithms = []

    def load(self) -> bool:
        self._algorithms = self._discover_algorithms()
        return True

    def unload(self):
        pass

    def id(self) -> str:
        return 'geoengine'

    def name(self) -> str:
        return 'GeoEngine'

    def longName(self) -> str:
        return 'GeoEngine Containerized Geoprocessing'

    def icon(self):
        return QgsProcessingProvider.icon(self)

    def loadAlgorithms(self):
        for alg in self._algorithms:
            self.addAlgorithm(alg)

    def _discover_algorithms(self) -> List[QgsProcessingAlgorithm]:
        """Discover algorithms from geoengine CLI."""
        algorithms = []

        try:
            client = GeoEngineCLIClient()
            workers = client.list_workers()

            for worker in workers:
                if not worker.get('has_tool', False):
                    continue
                tool = client.get_worker_tool(worker['name'])
                if tool:
                    alg = GeoEngineAlgorithm(worker['name'], tool)
                    algorithms.append(alg)

        except Exception as e:
            print(f"GeoEngine tool discovery failed: {e}")

        return algorithms


# ---------------------------------------------------------------------------
# QGIS Processing Algorithm
# ---------------------------------------------------------------------------


class GeoEngineAlgorithm(QgsProcessingAlgorithm):
    """Dynamic QGIS Processing algorithm for a GeoEngine worker."""

    def __init__(self, worker_name: str, tool_info: Dict):
        super().__init__()
        self._worker = worker_name
        self._tool = tool_info
        self._inputs = tool_info.get('inputs', [])

    def createInstance(self):
        return GeoEngineAlgorithm(self._worker, self._tool)

    def name(self) -> str:
        return self._worker

    def displayName(self) -> str:
        return self._tool.get('name', self._worker)

    def group(self) -> str:
        return ''

    def groupId(self) -> str:
        return ''

    def shortHelpString(self) -> str:
        return self._tool.get('description', '')

    def initAlgorithm(self, config=None):
        """Define algorithm parameters from worker's input definitions."""
        for inp in self._inputs:
            param = self._create_parameter(inp)
            if param:
                self.addParameter(param)

    def _create_parameter(self, param_info: Dict):
        """Create a QGIS parameter from worker input parameter info."""
        param_type = param_info.get('param_type', 'string')
        name = param_info['name']
        label = param_info.get('description', name)
        required = param_info.get('required', True)
        default = param_info.get('default')
        enum_values = param_info.get('enum_values', [])

        if param_type == 'file':
            param = QgsProcessingParameterFile(name, label, optional=not required)
        elif param_type == 'folder':
            param = QgsProcessingParameterFile(
                name, label, behavior=QgsProcessingParameterFile.Folder, optional=not required
            )
        elif param_type == 'datetime':
            param = QgsProcessingParameterString(name, label, defaultValue=default, optional=not required)
        elif param_type == 'string':
            param = QgsProcessingParameterString(name, label, defaultValue=default, optional=not required)
        elif param_type == 'number':
            param = QgsProcessingParameterNumber(
                name, label, type=QgsProcessingParameterNumber.Double,
                defaultValue=default, optional=not required
            )
        elif param_type == 'boolean':
            param = QgsProcessingParameterBoolean(name, label, defaultValue=default or False, optional=not required)
        elif param_type == 'enum':
            if not enum_values:
                enum_values = []
            param = QgsProcessingParameterEnum(
                name, label, enum_values, defaultValue=default,
                optional=not required, usesStaticStrings=True
            )
        else:
            param = QgsProcessingParameterString(name, label, defaultValue=default, optional=not required)

        return param

    def processAlgorithm(
        self,
        parameters: Dict,
        context: QgsProcessingContext,
        feedback: QgsProcessingFeedback
    ) -> Dict:
        """Execute the algorithm via geoengine CLI."""
        client = GeoEngineCLIClient()

        inputs = {}
        for inp in self._inputs:
            name = inp['name']
            if name in parameters:
                value = parameters[name]
                if hasattr(value, 'source'):
                    value = value.source()
                elif hasattr(value, 'dataProvider'):
                    value = value.dataProvider().dataSourceUri()
                inputs[name] = str(value) if value is not None else None

        feedback.pushInfo(f"Running worker '{self._worker}'...")

        result = client.run_tool(
            worker=self._worker,
            inputs=inputs,
            on_output=lambda line: feedback.pushInfo(line),
            is_cancelled=lambda: feedback.isCanceled(),
        )

        feedback.pushInfo("Worker completed successfully!")
        feedback.setProgress(100)

        return {'status': result.get('status', 'completed')}
