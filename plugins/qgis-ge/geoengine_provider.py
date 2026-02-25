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
import tempfile
from typing import Any, Callable, Dict, List, Optional

from qgis.PyQt.QtCore import QUrl
from qgis.PyQt.QtGui import QDesktopServices
from qgis.core import (
    QgsMapLayerType,
    QgsProcessingAlgorithm,
    QgsProcessingContext,
    QgsProcessingFeedback,
    QgsProcessingParameterMapLayer,
    QgsProcessingParameterString,
    QgsProcessingParameterNumber,
    QgsProcessingParameterBoolean,
    QgsProcessingParameterFile,
    QgsProcessingParameterEnum,
    QgsProcessingProvider,
    QgsProject,
    QgsRasterFileWriter,
    QgsRasterLayer,
    QgsRasterPipe,
    QgsSettings,
    QgsVectorLayer,
    QgsVectorFileWriter,
)


# ---------------------------------------------------------------------------
# CLI Client
# ---------------------------------------------------------------------------

DEV_MODE_SETTING_KEY = "geoengine/dev_mode"


def is_dev_mode_enabled() -> bool:
    """Return whether QGIS settings enable GeoEngine dev image execution."""
    return QgsSettings().value(DEV_MODE_SETTING_KEY, False, type=bool)


def set_dev_mode_enabled(enabled: bool) -> None:
    """Persist the GeoEngine dev-mode setting in QGIS preferences."""
    s = QgsSettings()
    s.setValue(DEV_MODE_SETTING_KEY, enabled)
    s.sync()


class GeoEngineCLIClient:
    """Client that invokes the geoengine CLI binary via subprocess."""

    def __init__(self):
        """Resolve the GeoEngine binary location."""
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
        """Return a health payload containing the CLI version string."""
        result = subprocess.run(
            [self.binary, '--version'],
            capture_output=True, text=True, timeout=10
        )
        if result.returncode != 0:
            raise Exception(f"geoengine version check failed: {result.stderr.strip()}")
        return {'status': 'healthy', 'version': result.stdout.strip()}

    def list_workers(self) -> List[Dict]:
        """List worker descriptors available for QGIS integration."""
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
        dev_mode: Optional[bool] = None,
    ) -> Dict:
        """Run a worker synchronously, streaming container output via on_output.

        Input parameters are passed as --input KEY=VALUE flags.
        File paths are auto-mounted into the container.
        """
        cmd = [self.binary, 'run', worker, '--json']
        if dev_mode is None:
            dev_mode = is_dev_mode_enabled()
        if dev_mode:
            cmd.append('--dev')

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
        """Initialize the provider and its algorithm cache."""
        super().__init__()
        self._algorithms = []

    def load(self) -> bool:
        """Populate provider algorithms and report load success."""
        self._algorithms = self._discover_algorithms()
        return True

    def unload(self):
        """Unload provider resources (no-op)."""
        pass

    def id(self) -> str:
        """Return the stable provider identifier."""
        return 'geoengine'

    def name(self) -> str:
        """Return the short provider display name."""
        return 'GeoEngine'

    def longName(self) -> str:
        """Return the long provider display name."""
        return 'GeoEngine Containerized Geoprocessing'

    def icon(self):
        """Return the provider icon shown by QGIS."""
        return QgsProcessingProvider.icon(self)

    def loadAlgorithms(self):
        """Register cached algorithms with QGIS."""
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
    OPEN_OUTPUT_FOLDER_PARAM = "__geoengine_open_output_folder"

    def __init__(self, worker_name: str, tool_info: Dict):
        """Initialize an algorithm wrapper for a worker definition."""
        super().__init__()
        self._worker = worker_name
        self._tool = tool_info
        self._inputs = tool_info.get('inputs', [])

    def createInstance(self):
        """Create a new algorithm instance for QGIS cloning."""
        return GeoEngineAlgorithm(self._worker, self._tool)

    def name(self) -> str:
        """Return the algorithm id used by QGIS processing."""
        return self._worker

    def displayName(self) -> str:
        """Return the human-readable algorithm name."""
        return self._tool.get('name', self._worker)

    def group(self) -> str:
        """Return the group label for this algorithm."""
        return ''

    def groupId(self) -> str:
        """Return the group identifier for this algorithm."""
        return ''

    def shortHelpString(self) -> str:
        """Return help text sourced from the worker description."""
        return self._tool.get('description', '')

    def initAlgorithm(self, config=None):
        """Define algorithm parameters from worker's input definitions."""
        for inp in self._inputs:
            param = self._create_parameter(inp)
            if param:
                self.addParameter(param)
        self.addParameter(
            QgsProcessingParameterBoolean(
                self.OPEN_OUTPUT_FOLDER_PARAM,
                "Open output folder when complete",
                defaultValue=False,
                optional=True,
            )
        )

    def _create_parameter(self, param_info: Dict):
        """Create a QGIS parameter from worker input parameter info."""
        param_type = param_info.get('param_type', 'string')
        name = param_info['name']
        label = param_info.get('description', name)
        required = param_info.get('required', True)
        default = param_info.get('default')
        enum_values = param_info.get('enum_values', [])
        readonly = param_info.get('readonly', True)

        if param_type == 'file':
            if readonly:
                # Read-only file inputs are typically geospatial layers; let users
                # choose an existing layer or browse to an external layer source.
                param = QgsProcessingParameterMapLayer(name, label, optional=not required)
            else:
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

    @staticmethod
    def _strip_qgis_source_uri_suffix(source: str) -> str:
        """Strip QGIS provider URI suffixes like '|layername=foo' from local file sources."""
        if not source:
            return source
        if source.startswith("file://"):
            local = QUrl(source).toLocalFile()
            if local:
                source = local
        if '|' in source:
            source = source.split('|', 1)[0]
        return source

    @classmethod
    def _normalize_local_file_path(cls, path_or_uri: str) -> Optional[str]:
        """Normalize a local file path/URI; return None for non-local/unusable sources."""
        if not path_or_uri:
            return None
        source = cls._strip_qgis_source_uri_suffix(str(path_or_uri))
        if not source:
            return None
        if source.startswith(('/vsimem/', 'memory:')) or source.startswith('dbname='):
            return None
        if os.path.isfile(source):
            return os.path.realpath(source)
        return None

    @classmethod
    def _normalize_local_dir_path(cls, path_or_uri: str) -> Optional[str]:
        """Normalize a local directory path/URI; return None for non-local/unusable sources."""
        if not path_or_uri:
            return None
        source = cls._strip_qgis_source_uri_suffix(str(path_or_uri))
        if not source:
            return None
        if source.startswith(('/vsimem/', 'memory:')) or source.startswith('dbname='):
            return None
        if os.path.isdir(source):
            return os.path.realpath(source)
        return None

    @staticmethod
    def _result_file_entries(result: Dict) -> List[Dict[str, Any]]:
        """Extract normalized file entries from the CLI JSON result payload."""
        entries: List[Dict[str, Any]] = []
        seen = set()
        for entry in result.get('files', []) or []:
            if isinstance(entry, str):
                entry = {'path': entry}
            if not isinstance(entry, dict):
                continue
            path = entry.get('path')
            kind = str(entry.get('kind') or 'output')
            if isinstance(entry, dict):
                path = entry.get('path')
            if not path:
                continue
            path = str(path)
            key = (path, kind)
            if key in seen:
                continue
            seen.add(key)
            entries.append({
                'path': path,
                'kind': kind,
                'name': entry.get('name'),
                'size': entry.get('size'),
            })
        return entries

    @staticmethod
    def _parameter_bool(parameters: Dict, name: str, default: bool = False) -> bool:
        """Coerce a processing parameter value to bool."""
        if name not in parameters:
            return default
        value = parameters.get(name)
        if value is None:
            return default
        if isinstance(value, bool):
            return value
        if isinstance(value, (int, float)):
            return bool(value)
        return str(value).strip().lower() in ("1", "true", "yes", "y", "on")

    @classmethod
    def _project_loaded_file_paths(cls) -> set:
        """Return normalized local file paths currently loaded in the project."""
        loaded = set()
        for layer in QgsProject.instance().mapLayers().values():
            try:
                source = layer.source()
            except Exception:
                continue
            normalized = cls._normalize_local_file_path(source)
            if normalized:
                loaded.add(normalized)
        return loaded

    @staticmethod
    def _is_supported_output_file(path: str) -> bool:
        """Return True for likely GIS layer files and False for common sidecars."""
        lower_name = os.path.basename(path).lower()
        if lower_name.endswith(('.aux.xml', '.ovr', '.prj', '.shx', '.dbf', '.cpg', '.qix', '.sbn', '.sbx')):
            return False

        ext = os.path.splitext(lower_name)[1]
        supported = {
            '.shp', '.gpkg', '.geojson', '.json', '.kml', '.gml', '.sqlite',
            '.tif', '.tiff', '.vrt', '.img', '.jp2', '.asc'
        }
        return ext in supported

    @staticmethod
    def _try_load_output_layer(path: str, context: QgsProcessingContext) -> Optional[str]:
        """Try loading a CLI output file as a raster or vector layer.

        Instead of adding the layer directly to the project, registers it with
        *context* via ``addLayerToLoadOnCompletion`` so that QGIS loads it
        safely after the algorithm finishes.
        """
        if not path or not os.path.isfile(path):
            return None
        if not GeoEngineAlgorithm._is_supported_output_file(path):
            return None

        layer_name = os.path.splitext(os.path.basename(path))[0] or os.path.basename(path)
        ext = os.path.splitext(path.lower())[1]

        raster_exts = {'.tif', '.tiff', '.vrt', '.img', '.jp2', '.asc'}
        vector_exts = {'.shp', '.gpkg', '.geojson', '.json', '.kml', '.gml', '.sqlite'}

        candidates = []
        if ext in raster_exts:
            candidates = [('raster', QgsRasterLayer(path, layer_name))]
        elif ext in vector_exts:
            candidates = [('vector', QgsVectorLayer(path, layer_name, 'ogr'))]
        else:
            candidates = [
                ('raster', QgsRasterLayer(path, layer_name)),
                ('vector', QgsVectorLayer(path, layer_name, 'ogr')),
            ]

        for layer_type, layer in candidates:
            if layer and layer.isValid():
                details = QgsProcessingContext.LayerDetails(layer_name, context.project())
                context.addLayerToLoadOnCompletion(layer.id(), details)
                context.temporaryLayerStore().addMapLayer(layer)
                return layer_type

        return None

    @staticmethod
    def _safe_temp_stem(name: str) -> str:
        """Build a filesystem-safe stem for temp exports."""
        raw = (name or "input").strip()
        cleaned = "".join(ch if ch.isalnum() or ch in ("-", "_") else "_" for ch in raw)
        return cleaned or "input"

    def _export_vector_layer_to_temp(
        self,
        layer,
        input_name: str,
        feedback: QgsProcessingFeedback,
    ) -> (str, str):
        """Export a vector layer to a temporary GeoPackage and return (path, temp_dir)."""
        temp_dir = tempfile.mkdtemp(prefix="geoengine-qgis-input-")
        out_path = os.path.join(temp_dir, f"{self._safe_temp_stem(input_name)}.gpkg")
        feedback.pushInfo(f"Exporting non-file-backed vector input '{input_name}' to temp file...")

        options = QgsVectorFileWriter.SaveVectorOptions()
        options.driverName = "GPKG"
        options.fileEncoding = "UTF-8"
        result = QgsVectorFileWriter.writeAsVectorFormatV3(
                layer,
                out_path,
                QgsProject.instance().transformContext(),
                options,
        )
        err_code = result[0] if isinstance(result, tuple) else result

        if err_code != QgsVectorFileWriter.NoError:
            shutil.rmtree(temp_dir, ignore_errors=True)
            raise Exception(f"Failed to export vector input '{input_name}' to temporary GeoPackage")

        return out_path, temp_dir

    def _export_raster_layer_to_temp(
        self,
        layer,
        input_name: str,
        context: QgsProcessingContext,
        feedback: QgsProcessingFeedback,
    ) -> (str, str):
        """Export a raster layer to a temporary GeoTIFF and return (path, temp_dir)."""
        temp_dir = tempfile.mkdtemp(prefix="geoengine-qgis-input-")
        out_path = os.path.join(temp_dir, f"{self._safe_temp_stem(input_name)}.tif")
        feedback.pushInfo(f"Exporting non-file-backed raster input '{input_name}' to temp file...")

        provider = layer.dataProvider()
        if provider is None:
            shutil.rmtree(temp_dir, ignore_errors=True)
            raise Exception(f"Raster input '{input_name}' has no data provider")

        pipe = QgsRasterPipe()
        provider_clone = provider.clone()
        if provider_clone is None or not pipe.set(provider_clone):
            shutil.rmtree(temp_dir, ignore_errors=True)
            raise Exception(f"Failed to prepare raster pipe for input '{input_name}'")

        writer = QgsRasterFileWriter(out_path)
        writer.setOutputFormat("GTiff")
        result = writer.writeRaster(
            pipe,
            provider.xSize(),
            provider.ySize(),
            provider.extent(),
            provider.crs(),
            context.transformContext(),
        )
        no_error = getattr(QgsRasterFileWriter, "NoError", 0)
        if result != no_error:
            shutil.rmtree(temp_dir, ignore_errors=True)
            raise Exception(f"Failed to export raster input '{input_name}' to temporary GeoTIFF")

        return out_path, temp_dir

    def _export_layer_to_temp_if_needed(
        self,
        layer,
        input_name: str,
        context: QgsProcessingContext,
        feedback: QgsProcessingFeedback,
    ) -> Optional[Dict[str, str]]:
        """Export a non-file-backed layer to a temp file and return metadata."""
        try:
            layer_type = layer.type()
        except Exception:
            layer_type = None

        if layer_type == QgsMapLayerType.VectorLayer:
            out_path, temp_dir = self._export_vector_layer_to_temp(layer, input_name, feedback)
            return {"path": out_path, "temp_dir": temp_dir}
        if layer_type == QgsMapLayerType.RasterLayer:
            out_path, temp_dir = self._export_raster_layer_to_temp(layer, input_name, context, feedback)
            return {"path": out_path, "temp_dir": temp_dir}
        raise Exception(f"Unsupported map layer type for input '{input_name}'")

    def _resolve_readonly_file_input(
        self,
        name: str,
        parameters: Dict,
        context: QgsProcessingContext,
        feedback: QgsProcessingFeedback,
    ) -> Dict[str, Any]:
        """Resolve a read-only file input from a map-layer parameter to a local path."""
        layer = self.parameterAsLayer(parameters, name, context)
        if layer is not None:
            source = None
            try:
                source = layer.source()
            except Exception:
                source = None
            if not source and hasattr(layer, 'dataProvider') and layer.dataProvider():
                try:
                    source = layer.dataProvider().dataSourceUri()
                except Exception:
                    source = None
            source_text = str(source or "")
            # Provider URIs with sublayer selectors (e.g. GeoPackage layername)
            # should preserve the selected layer, so export instead of stripping.
            if "|" in source_text:
                normalized = None
            else:
                normalized = self._normalize_local_file_path(source_text)
            if normalized:
                return {"path": normalized, "temp_dir": None, "exported_temp": False}
            exported = self._export_layer_to_temp_if_needed(layer, name, context, feedback)
            if exported:
                return {"path": exported["path"], "temp_dir": exported["temp_dir"], "exported_temp": True}
            return {"path": str(source) if source else None, "temp_dir": None, "exported_temp": False}

        raw_value = parameters.get(name)
        if raw_value is None:
            return {"path": None, "temp_dir": None, "exported_temp": False}
        raw_text = str(raw_value)
        normalized = self._normalize_local_file_path(raw_text)
        return {"path": normalized or raw_text, "temp_dir": None, "exported_temp": False}

    def _maybe_open_output_folder(
        self,
        output_dirs: List[str],
        feedback: QgsProcessingFeedback,
        enabled: bool,
    ) -> None:
        """Open all deduped output directories when enabled for this run."""
        if not enabled:
            return

        if not output_dirs:
            return

        for folder in output_dirs:
            if not folder or not os.path.isdir(folder):
                continue
            ok = QDesktopServices.openUrl(QUrl.fromLocalFile(folder))
            if ok:
                feedback.pushInfo(f"Opened output folder: {folder}")
            else:
                feedback.pushInfo(f"Could not open output folder: {folder}")

    def _output_dirs_from_parameters(
        self,
        parameters: Dict,
        context: QgsProcessingContext,
        feedback: QgsProcessingFeedback,
    ) -> List[str]:
        """Resolve writable output directories from command input parameters."""
        dirs: List[str] = []
        seen = set()

        for inp in self._inputs:
            param_type = str(inp.get('param_type', 'string')).lower()
            readonly = inp.get('readonly', True)
            if readonly or param_type not in ('file', 'folder'):
                continue

            name = inp.get('name')
            if not name or name not in parameters:
                continue

            value = parameters.get(name)
            if value is None:
                continue

            if hasattr(value, 'source'):
                value = value.source()
            elif hasattr(value, 'dataProvider'):
                provider = value.dataProvider()
                if provider is not None:
                    value = provider.dataSourceUri()

            path_text = str(value).strip()
            if not path_text:
                continue

            if param_type == 'file':
                # Use the parent directory for writable file targets.
                if path_text.startswith("file://"):
                    path_text = QUrl(path_text).toLocalFile() or path_text
                folder = os.path.dirname(path_text)
                if not folder:
                    feedback.pushInfo(f"Skipping output file input '{name}' with no parent directory: {path_text}")
                    continue
                normalized = os.path.realpath(folder)
            else:
                normalized = self._normalize_local_dir_path(path_text) or os.path.realpath(path_text)

            if normalized in seen:
                continue
            seen.add(normalized)
            dirs.append(normalized)

        return dirs

    def processAlgorithm(
        self,
        parameters: Dict,
        context: QgsProcessingContext,
        feedback: QgsProcessingFeedback
    ) -> Dict:
        """Execute the algorithm via geoengine CLI."""
        client = GeoEngineCLIClient()

        inputs = {}
        preloaded_project_paths = self._project_loaded_file_paths()
        temp_export_dirs: List[str] = []
        temp_export_input_paths: set = set()
        for inp in self._inputs:
            name = inp['name']
            if name in parameters:
                param_type = inp.get('param_type', 'string')
                readonly = inp.get('readonly', True)
                if param_type == 'file' and readonly:
                    resolved = self._resolve_readonly_file_input(name, parameters, context, feedback)
                    value = resolved.get("path")
                    if resolved.get("temp_dir"):
                        temp_export_dirs.append(str(resolved["temp_dir"]))
                    if resolved.get("exported_temp") and value:
                        normalized = self._normalize_local_file_path(str(value))
                        temp_export_input_paths.add(normalized or str(value))
                else:
                    value = parameters[name]
                    if hasattr(value, 'source'):
                        value = value.source()
                    elif hasattr(value, 'dataProvider'):
                        provider = value.dataProvider()
                        if provider is not None:
                            value = provider.dataSourceUri()
                inputs[name] = str(value) if value is not None else None

        feedback.pushInfo(f"Running worker '{self._worker}'...")
        open_output_folder = self._parameter_bool(
            parameters,
            self.OPEN_OUTPUT_FOLDER_PARAM,
            default=False,
        )
        requested_output_dirs = self._output_dirs_from_parameters(parameters, context, feedback)

        try:
            result = client.run_tool(
                worker=self._worker,
                inputs=inputs,
                on_output=lambda line: feedback.pushInfo(line),
                is_cancelled=lambda: feedback.isCanceled(),
            )

            feedback.pushInfo("Worker completed successfully!")
            feedback.setProgress(100)

            raw_file_entries = self._result_file_entries(result)
            file_entries = []
            for entry in raw_file_entries:
                path = entry.get('path')
                kind = entry.get('kind', 'output')
                normalized = self._normalize_local_file_path(path or "") or str(path or "")
                if kind == 'input' and normalized in temp_export_input_paths:
                    # Temp export is an internal bridge for non-file-backed layers.
                    continue
                file_entries.append(entry)

            loaded_paths = []
            for entry in file_entries:
                path = entry.get('path')
                kind = entry.get('kind', 'output')
                normalized = self._normalize_local_file_path(path or "")
                if kind == 'input' and normalized and normalized in preloaded_project_paths:
                    feedback.pushInfo(f"Input layer already loaded in project: {path}")
                    continue
                loaded_type = self._try_load_output_layer(path, context)
                if loaded_type:
                    loaded_paths.append(path)
                    feedback.pushInfo(f"Loaded {loaded_type} {kind} layer: {path}")
                else:
                    feedback.pushInfo(f"{kind.title()} file produced (not auto-loaded): {path}")

            self._maybe_open_output_folder(requested_output_dirs, feedback, open_output_folder)

            response: Dict[str, Any] = {'status': result.get('status', 'completed')}
            if file_entries:
                response['files'] = [entry.get('path') for entry in file_entries if entry.get('path')]
            if loaded_paths:
                response['loaded_files'] = loaded_paths
            return response
        finally:
            for temp_dir in temp_export_dirs:
                shutil.rmtree(temp_dir, ignore_errors=True)
