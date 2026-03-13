# -*- coding: utf-8 -*-
"""
GeoEngine QGIS Processing Provider
Invokes the geoengine CLI directly to provide containerized geoprocessing tools
as QGIS Processing algorithms.
"""

import json
import os
import qgis.utils
import shutil
import subprocess
import tempfile
import traceback
from typing import Any, Callable, Dict, List, Optional, Tuple

from qgis.PyQt.QtCore import QUrl
from qgis.PyQt.QtGui import QDesktopServices
from qgis.core import (
    QgsMapLayerType,
    QgsProcessingAlgorithm,
    QgsProcessingContext,
    QgsProcessingFeedback,
    QgsProcessingParameterString,
    QgsProcessingParameterNumber,
    QgsProcessingParameterBoolean,
    QgsProcessingParameterFile,
    QgsProcessingParameterFileDestination,
    QgsProcessingParameterFolderDestination,
    QgsProcessingParameterEnum,
    QgsProcessingProvider,
    QgsMessageLog,
    QgsProject,
    QgsRasterFileWriter,
    QgsRasterLayer,
    QgsRasterPipe,
    QgsSettings,
    QgsVectorLayer,
    QgsVectorFileWriter,
)
from .geoengine_widgets import (
    MapLayerWithFileWrapper
)


# ---------------------------------------------------------------------------
# Plugin-local temp directory
# ---------------------------------------------------------------------------
# All mkdtemp calls use this as their base directory so that temporary files
# live inside the plugin folder rather than the OS temp root (e.g. macOS
# /var/folders/…).  The plugin folder is already bind-mounted into Docker, so
# any sub-directory is accessible to the container without extra configuration.
_PLUGIN_DIR = os.path.dirname(os.path.abspath(__file__))
_PLUGIN_TMP_DIR = os.path.join(_PLUGIN_DIR, "tmp")
os.makedirs(_PLUGIN_TMP_DIR, exist_ok=True)

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
        """Locate the geoengine binary across platforms.

        QGIS launched as a GUI app (e.g. from the macOS dock or Windows start
        menu) inherits a minimal PATH that often omits the directories where
        package managers and Cargo install binaries.  This method tries a broad
        set of well-known locations before giving up.
        """
        import platform
        is_windows = platform.system() == "Windows"
        binary_name = "geoengine.exe" if is_windows else "geoengine"

        # --- 1. Augment PATH with common install locations and try shutil.which ---
        extra_dirs: List[str] = []
        home = os.path.expanduser('~')

        if is_windows:
            local_app_data = os.environ.get("LOCALAPPDATA", "")
            program_files = os.environ.get("ProgramFiles", r"C:\Program Files")
            program_files_x86 = os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")
            extra_dirs = [
                os.path.join(home, '.geoengine', 'bin'),
                os.path.join(home, '.cargo', 'bin'),
                os.path.join(local_app_data, 'Programs', 'geoengine', 'bin') if local_app_data else '',
                os.path.join(program_files, 'GeoEngine', 'bin'),
                os.path.join(program_files_x86, 'GeoEngine', 'bin'),
            ]
        else:
            extra_dirs = [
                os.path.join(home, '.geoengine', 'bin'),
                os.path.join(home, '.cargo', 'bin'),
                '/usr/local/bin',
                '/usr/bin',
                '/opt/homebrew/bin',
                '/opt/homebrew/sbin',
                '/opt/local/bin',
                '/snap/bin',
            ]

        current_path = os.environ.get("PATH", "")
        path_sep = ";" if is_windows else ":"
        path_parts = current_path.split(path_sep) if current_path else []
        augmented_parts = list(path_parts)
        for d in extra_dirs:
            if d and d not in augmented_parts:
                augmented_parts.append(d)
        augmented_path = path_sep.join(augmented_parts)

        found = shutil.which(binary_name, path=augmented_path)
        if not found and not is_windows:
            # shutil.which on some systems won't find "geoengine.exe"-less names
            found = shutil.which('geoengine', path=augmented_path)
        if found:
            # Persist the augmented PATH so subprocess calls inherit it too.
            os.environ["PATH"] = augmented_path
            return found

        # --- 2. Direct filesystem probe for well-known absolute paths ---
        candidates: List[str] = []
        if is_windows:
            local_app_data = os.environ.get("LOCALAPPDATA", "")
            program_files = os.environ.get("ProgramFiles", r"C:\Program Files")
            program_files_x86 = os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")
            candidates = [
                os.path.join(home, '.geoengine', 'bin', 'geoengine.exe'),
                os.path.join(home, '.cargo', 'bin', 'geoengine.exe'),
                os.path.join(local_app_data, 'Programs', 'geoengine', 'bin', 'geoengine.exe') if local_app_data else '',
                os.path.join(program_files, 'GeoEngine', 'bin', 'geoengine.exe'),
                os.path.join(program_files_x86, 'GeoEngine', 'bin', 'geoengine.exe'),
            ]
        else:
            candidates = [
                os.path.join(home, '.geoengine', 'bin', 'geoengine'),
                os.path.join(home, '.cargo', 'bin', 'geoengine'),
                '/usr/local/bin/geoengine',
                '/opt/homebrew/bin/geoengine',
                '/opt/local/bin/geoengine',
                '/snap/bin/geoengine',
            ]

        for candidate in candidates:
            if not candidate:
                continue
            if is_windows:
                if os.path.isfile(candidate):
                    return candidate
            else:
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

    def get_worker_tool(self, name: str, ver: Optional[str] = None) -> Optional[Dict]:
        """Get the tool/input description for a worker via `geoengine run <name> --describe`."""
        describe_args = [self.binary, 'describe', name, '--json']
        if is_dev_mode_enabled():
            describe_args.append('--dev')
        elif ver:
            describe_args.extend(['--ver', ver])
        result = subprocess.run(
            describe_args,
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
        ver: Optional[str] = None,
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
        elif ver:
            cmd.extend(['--ver', ver])

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
                    forced_kill = False
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
                        forced_kill = True

                    tail_lines = []
                    if process.stderr:
                        remaining_stderr = process.stderr.read()
                        if remaining_stderr:
                            for raw in remaining_stderr.splitlines():
                                stripped_tail = raw.strip()
                                if not stripped_tail:
                                    continue
                                tail_lines.append(stripped_tail)
                                if on_output:
                                    on_output(stripped_tail)

                    if forced_kill:
                        detail = (
                            "geoengine subprocess did not exit after cancellation request "
                            "and was force-killed"
                        )
                    elif tail_lines:
                        detail = tail_lines[-1]
                    else:
                        detail = "no additional cancellation details"
                    error_msg = f"Job cancelled by user ({detail})"
                    if on_output:
                        on_output(f"ERROR: {error_msg}")
                    raise Exception(error_msg)

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

                if is_dev_mode_enabled():
                    # Dev mode: single algorithm per worker, no version grouping
                    if not worker.get('has_dev_image'):
                        continue
                    tool = client.get_worker_tool(worker['name'])
                    if tool:
                        algorithms.append(GeoEngineAlgorithm(worker['name'], tool, ver=None))
                else:
                    # Release mode: one algorithm per available version, grouped by worker
                    if not worker.get('has_pushed_image'):
                        continue
                    tool = client.get_worker_tool(worker['name'])
                    if not tool:
                        continue

                    available_versions = tool.get('available_versions') or []

                    if not available_versions:
                        # Fallback: worker has a pushed image but no version metadata yet;
                        # surface it as a single unversioned entry so it appears in the toolbox.
                        versioned_tool = client.get_worker_tool(worker['name']) or tool
                        algorithms.append(GeoEngineAlgorithm(worker['name'], versioned_tool, ver=None))
                    else:
                        for ver in available_versions:
                            versioned_tool = client.get_worker_tool(worker['name'], ver=ver) or tool
                            algorithms.append(GeoEngineAlgorithm(worker['name'], versioned_tool, ver=ver))

        except Exception as e:
            QgsMessageLog.logMessage(
                f"GeoEngine tool discovery failed: {e}",
                "GeoEngine",
                level=1,
            )
            try:
                iface = getattr(qgis.utils, "iface", None)
                if iface is not None:
                    iface.messageBar().pushWarning(
                        "GeoEngine",
                        "GeoEngine tool discovery failed. See Log Messages for details.",
                    )
            except Exception:
                pass

        return algorithms


# ---------------------------------------------------------------------------
# QGIS Processing Algorithm
# ---------------------------------------------------------------------------


class GeoEngineAlgorithm(QgsProcessingAlgorithm):
    """Dynamic QGIS Processing algorithm for a GeoEngine worker."""
    OPEN_OUTPUT_FOLDER_PARAM = "__geoengine_open_output_folder"

    def __init__(self, worker_name: str, tool_info: Dict, ver: Optional[str] = None, dev_mode: Optional[bool] = None):
        """Initialize an algorithm wrapper for a worker definition."""
        super().__init__()
        self._worker = worker_name
        self._tool = tool_info
        self._inputs = tool_info.get('inputs', [])
        self._ver = ver  # None means "latest" (no --ver flag)
        # Freeze dev mode and group membership at construction time so createInstance()
        # clones always return the same values — QGIS requires these to be stable.
        self._dev_mode = is_dev_mode_enabled() if dev_mode is None else dev_mode
        self._group = '' if self._dev_mode else tool_info.get('name', worker_name)
        self._group_id = '' if self._dev_mode else worker_name

    def createInstance(self):
        """Create a new algorithm instance for QGIS cloning."""
        return GeoEngineAlgorithm(self._worker, self._tool, self._ver, dev_mode=self._dev_mode)

    def name(self) -> str:
        """Return the algorithm id used by QGIS processing (must be unique per algorithm)."""
        if self._ver:
            safe = self._ver.replace('.', '_').replace('-', '_')
            return f"{self._worker}__ver__{safe}"
        return self._worker

    def displayName(self) -> str:
        """Return the human-readable algorithm name."""
        base = self._tool.get('name', self._worker)
        if self._ver:
            return f"{base} ({self._ver})"
        return base

    def group(self) -> str:
        """Return the group label for this algorithm.

        In release mode all versions of a worker share the worker's display
        name as their group, which makes QGIS render them under a collapsible
        labelled with the worker name in the Processing Toolbox.
        In dev mode there is only one algorithm per worker so no group is used.
        """
        return self._group

    def groupId(self) -> str:
        """Return the stable group identifier for this algorithm."""
        return self._group_id

    def shortHelpString(self) -> str:
        """Return help text sourced from the worker description, with apply/build metadata."""
        parts = []

        description = self._tool.get('description', '')
        if description:
            parts.append(description)

        if self._dev_mode:
            applied_at = self._tool.get('applied_at')
            built_at = self._tool.get('built_at')
            yaml_hash = self._tool.get('yaml_hash')
            script_hash = self._tool.get('script_hash')

            parts.append("\n--------------\n")
            apply_block = []
            if yaml_hash:
                apply_block.append(f"Saved YAML hash: {yaml_hash}...")
            if applied_at:
                apply_block.append(f"Last applied {self._format_age(applied_at)}")
            if apply_block:
                parts.append("\n".join(apply_block))
                parts.append("--------------\n")

            build_block = []
            if script_hash:
                build_block.append(f"Built script hash: {script_hash}...")
            if built_at:
                build_block.append(f"Last built {self._format_age(built_at)}")
            if build_block:
                parts.append("\n".join(build_block))
                parts.append("--------------\n")

        else:
            ver_display = self._ver or self._tool.get('version') or 'latest'
            parts.append(f"Version: {ver_display}")

        return "\n\n".join(parts)

    @staticmethod
    def _format_age(iso_ts: str) -> str:
        """Return a human-readable age string for an RFC3339 timestamp.

        Examples:
            "45s ago"
            "3min 12s ago"
            "over an hour ago"
        """
        try:
            from datetime import datetime, timezone
            # Strip fractional seconds, then remove any trailing timezone offset
            # (Z, +HH:MM, or -HH:MM) so strptime receives a bare local-time string.
            ts = iso_ts.split('.')[0].rstrip('Z')
            for sep in ('+', '-'):
                if sep in ts[10:]:  # skip date separators before position 10
                    ts = ts[:ts.index(sep, 10)]
                    break
            dt_utc = datetime.strptime(ts, "%Y-%m-%dT%H:%M:%S").replace(tzinfo=timezone.utc)
            now = datetime.now(timezone.utc)
            delta = max(0, int((now - dt_utc).total_seconds()))
            if delta >= 3600:
                return "over an hour ago"
            minutes, seconds = divmod(delta, 60)
            if minutes > 0:
                return f"{minutes}min {seconds}s ago"
            return f"{seconds}s ago"
        except Exception:
            return iso_ts

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
        filetypes: List[str] = param_info.get('filetypes') or []
        # Exclude the wildcard sentinel — an empty list means "all types".
        filetypes = [ft for ft in filetypes if ft != ".*"]
        # Build a Qt file-dialog filter string, e.g. "*.tif *.tiff" → "*.tif *.tiff"
        # QgsProcessingParameterFileDestination expects a filter like "GeoTIFF (*.tif *.tiff)"
        file_filter = (
            " ".join(f"*{ft}" for ft in filetypes) if filetypes else ""
        )

        if param_type == 'file':
            if readonly:
                # Read-only file inputs may be geospatial layers or plain files
                # (e.g. .txt, .csv). Use a string parameter so QGIS does not
                # validate the value as a map layer — our custom widget returns
                # either a layer source URI or a plain file path, both of which
                # are valid strings.
                param = QgsProcessingParameterString(
                    name,
                    label,
                    defaultValue=default,
                    optional=not required,
                )
                param.setMetadata({
                    'widget_wrapper': {
                        'class': MapLayerWithFileWrapper,
                        'filetypes': filetypes,  # e.g. ['.txt', '.csv'] or [] for all
                    }
                })
            else:
                param = QgsProcessingParameterFileDestination(
                    name, label,
                    fileFilter=file_filter if file_filter else "All files (*)",
                    defaultValue=default,
                    optional=not required,
                )
        elif param_type == 'folder':
            if readonly:
                param = QgsProcessingParameterFile(
                    name, label, behavior=QgsProcessingParameterFile.Folder,
                    defaultValue=default, optional=not required,
                )
            else:
                param = QgsProcessingParameterFolderDestination(name, label, defaultValue=default, optional=not required)
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
        """Return True for likely GIS layer files and False for common sidecars.

        Files with no extension are passed through for a blind probe attempt in
        _try_load_output_layer — QGIS will try both raster and vector drivers
        and silently discard it if neither succeeds.
        """
        lower_name = os.path.basename(path).lower()
        if lower_name.endswith(('.aux.xml', '.ovr', '.prj', '.shx', '.dbf', '.cpg', '.qix', '.sbn', '.sbx')):
            return False

        ext = os.path.splitext(lower_name)[1]
        if not ext:
            # No extension — let _try_load_output_layer probe it.
            return True
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
        context: QgsProcessingContext,
        feedback: QgsProcessingFeedback,
        suffix: str = ".gpkg",
    ) -> Tuple[str, str]:
        """Export a vector layer to a temporary file and return (path, temp_dir).

        suffix controls the output format (default .gpkg).  Pass a different
        extension (e.g. ".geojson") when the target worker only accepts that type.
        """
        temp_dir = tempfile.mkdtemp(prefix="geoengine-qgis-input-", dir=_PLUGIN_TMP_DIR)
        out_path = os.path.join(temp_dir, f"{self._safe_temp_stem(input_name)}{suffix}")
        feedback.pushInfo(f"Exporting non-file-backed vector input '{input_name}' to temp file...")

        # Map extension → OGR driver name.
        _VECTOR_DRIVERS = {
            ".gpkg": "GPKG",
            ".geojson": "GeoJSON",
            ".json": "GeoJSON",
            ".shp": "ESRI Shapefile",
            ".kml": "KML",
            ".gml": "GML",
            ".sqlite": "SQLite",
        }
        driver = _VECTOR_DRIVERS.get(suffix.lower(), "GPKG")
        options = QgsVectorFileWriter.SaveVectorOptions()
        options.driverName = driver
        options.fileEncoding = "UTF-8"
        result = QgsVectorFileWriter.writeAsVectorFormatV3(
                layer,
                out_path,
                context.transformContext(),
                options,
        )
        err_code = result[0] if isinstance(result, tuple) else result

        if err_code != QgsVectorFileWriter.NoError:
            shutil.rmtree(temp_dir, ignore_errors=True)
            raise Exception(f"Failed to export vector input '{input_name}' to temporary {driver} file")

        return out_path, temp_dir

    def _export_raster_layer_to_temp(
        self,
        layer,
        input_name: str,
        context: QgsProcessingContext,
        feedback: QgsProcessingFeedback,
    ) -> Tuple[str, str]:
        """Export a raster layer to a temporary GeoTIFF and return (path, temp_dir)."""
        temp_dir = tempfile.mkdtemp(prefix="geoengine-qgis-input-", dir=_PLUGIN_TMP_DIR)
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
        vector_suffix: str = ".gpkg",
    ) -> Optional[Dict[str, str]]:
        """Export a non-file-backed layer to a temp file and return metadata.

        vector_suffix: desired output extension for vector layers (e.g. ".geojson").
        Raster layers are always exported as GeoTIFF.
        """
        try:
            layer_type = layer.type()
        except Exception:
            layer_type = None

        if layer_type == QgsMapLayerType.VectorLayer:
            out_path, temp_dir = self._export_vector_layer_to_temp(layer, input_name, context, feedback, suffix=vector_suffix)
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
        filetypes: Optional[List[str]] = None,
    ) -> Dict[str, Any]:
        """Resolve a read-only file input from a map-layer parameter to a local path.

        filetypes: extensions accepted by the worker (e.g. [".geojson"]).  Used to
        pick the right export format when the source layer is not already in an
        accepted format (e.g. a no-extension QGIS scratch layer).
        """
        # Determine the preferred vector export suffix from the worker's accepted
        # filetypes.  Use the first declared type that maps to an OGR-writable
        # vector format; fall back to .gpkg if none match.
        _VECTOR_EXTS = {".gpkg", ".geojson", ".json", ".shp", ".kml", ".gml", ".sqlite"}
        accepted = [str(ft).lower() for ft in (filetypes or []) if ft != ".*"]
        preferred_vector_suffix = next(
            (ft for ft in accepted if ft.lower() in _VECTOR_EXTS),
            ".gpkg",
        )

        layer = self._parameter_as_layer_or_file(parameters, name, context)
        if layer is not None:
            source = None
            if isinstance(layer, str):
                source = layer
            else:
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
            if "|" in source_text and not isinstance(layer, str):
                normalized = None
            else:
                normalized = self._normalize_local_file_path(source_text)
            # Only use the file path directly when it already has an extension that
            # the worker accepts (or when no filetypes restriction is declared).
            # A no-extension path (QGIS scratch layer) must be exported so the CLI
            # receives a properly named file in the expected format.
            if normalized:
                ext = os.path.splitext(normalized)[1].lower()
                if ext and (not accepted or ext in accepted):
                    return {"path": normalized, "temp_dir": None, "exported_temp": False}
            if not isinstance(layer, str):
                exported = self._export_layer_to_temp_if_needed(
                    layer, name, context, feedback, vector_suffix=preferred_vector_suffix
                )
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
        resolved_writable_files: Optional[Dict[str, str]] = None,
    ) -> List[str]:
        """Resolve writable output directories from command input parameters.

        resolved_writable_files: pre-resolved paths for writable file inputs
        (keyed by input name) so TEMPORARY_OUTPUT is not re-encountered here.
        """
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

            # Use the pre-resolved path for writable file inputs if available,
            # to avoid re-encountering the raw "TEMPORARY_OUTPUT" sentinel.
            if resolved_writable_files and name in resolved_writable_files:
                path_text = resolved_writable_files[name]
            else:
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
                if path_text == "TEMPORARY_OUTPUT":
                    # Determine the output extension.
                    # Use filetypes only when exactly one type is declared — if
                    # multiple are listed we can't know which one the script will
                    # produce, so fall back to the default value's extension.
                    # Exclude the wildcard sentinel in all cases.
                    filetypes = [
                        ft for ft in (inp.get('filetypes') or []) if ft != ".*"
                    ]
                    if len(filetypes) == 1:
                        suffix = filetypes[0]
                    else:
                        suffix = os.path.splitext(str(inp.get('default') or ""))[1]
                    # Create a temporary *directory* and build a path inside it
                    # without touching the filesystem. This lets the script create
                    # the file itself (with the correct format), avoiding the
                    # mkstemp pitfall where a pre-created file blocks geospatial
                    # writers (GDAL, fiona, terra, etc.) that refuse to overwrite
                    # or misinterpret an already-existing empty file.
                    tmp_dir = tempfile.mkdtemp(prefix="geoengine-qgis-output-", dir=_PLUGIN_TMP_DIR)
                    os.chmod(tmp_dir, 0o700)  # owner-only; Docker mounts inherit host UID
                    path_text = os.path.join(tmp_dir, f"output{suffix}")
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

    def _parameter_as_layer_or_file(self, parameters, name, context):
        value = parameters.get(name)
        if value is None:
            return None
        if isinstance(value, str):
            # Try to resolve the value as a project layer first, so
            # file-backed layers are returned as layer objects (enabling
            # downstream export logic) rather than plain file paths.
            layer = QgsProject.instance().mapLayer(value)
            if layer is not None:
                return layer
            # Search by source URI (e.g. the combo box may store the layer's
            # source path rather than its layer ID).
            stripped = self._strip_qgis_source_uri_suffix(value)
            stripped_match = None
            for lyr in QgsProject.instance().mapLayers().values():
                try:
                    lyr_source_raw = lyr.source()
                    if lyr_source_raw == value:
                        return lyr
                    lyr_source = self._strip_qgis_source_uri_suffix(lyr_source_raw)
                    if stripped_match is None and lyr_source == stripped:
                        stripped_match = lyr
                except Exception as e:
                    QgsMessageLog.logMessage(
                        f"GeoEngine: skipping layer '{lyr.id()}' ({lyr.name()}) during source URI"
                        f" lookup: {e}\n{traceback.format_exc()}",
                        "GeoEngine",
                        0,
                    )
            if stripped_match is not None:
                return stripped_match
            # No matching project layer — if the string is an existing file
            # path, return it directly.  This avoids calling parameterAsLayer
            # on a plain file (e.g. .txt, .csv) which can cause QGIS to load
            # it as a delimited-text layer whose .source() returns a mangled
            # URI with query parameters rather than the clean path the worker
            # expects.
            if os.path.isfile(stripped):
                return value
        # For non-string values (e.g. QgsMapLayer objects passed directly),
        # return as-is.
        if hasattr(value, 'source'):
            return value
        return value

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
        # Maps writable file input name -> resolved real path, so that
        # _output_dirs_from_parameters can use the already-resolved paths
        # instead of re-reading the raw (possibly "TEMPORARY_OUTPUT") parameter.
        resolved_writable_file_paths: Dict[str, str] = {}

        for inp in self._inputs:
            name = inp['name']
            if name in parameters:
                param_type = inp.get('param_type', 'string')
                readonly = inp.get('readonly', True)
                if param_type == 'file' and readonly:
                    inp_filetypes = [ft for ft in (inp.get('filetypes') or []) if ft != ".*"]
                    resolved = self._resolve_readonly_file_input(name, parameters, context, feedback, filetypes=inp_filetypes)
                    value = resolved.get("path")
                    if resolved.get("temp_dir"):
                        temp_export_dirs.append(str(resolved["temp_dir"]))
                    if resolved.get("exported_temp") and value:
                        normalized = self._normalize_local_file_path(str(value))
                        temp_export_input_paths.add(normalized or str(value))
                elif param_type == 'file' and not readonly:
                    # Writable file output: resolve TEMPORARY_OUTPUT to a real
                    # path now so geoengine run receives an actual filesystem path
                    # rather than the literal string "TEMPORARY_OUTPUT".
                    raw = parameters[name]
                    if hasattr(raw, 'source'):
                        raw = raw.source()
                    path_text = str(raw).strip() if raw is not None else ""
                    if path_text == "TEMPORARY_OUTPUT":
                        filetypes = [
                            ft for ft in (inp.get('filetypes') or []) if ft != ".*"
                        ]
                        suffix = filetypes[0] if len(filetypes) == 1 else (
                            os.path.splitext(str(inp.get('default') or ""))[1]
                        )
                        tmp_dir = tempfile.mkdtemp(prefix="geoengine-qgis-output-", dir=_PLUGIN_TMP_DIR)
                        os.chmod(tmp_dir, 0o700)
                        path_text = os.path.join(tmp_dir, f"output{suffix}")
                        feedback.pushInfo(
                            f"Using temporary output path for input '{name}': {path_text!r}"
                        )
                    value = path_text
                    resolved_writable_file_paths[name] = path_text
                elif param_type == 'folder' and not readonly:
                    # Writable folder output: resolve TEMPORARY_OUTPUT to a real
                    # directory so geoengine run receives an actual filesystem path.
                    raw = parameters[name]
                    if hasattr(raw, 'source'):
                        raw = raw.source()
                    path_text = str(raw).strip() if raw is not None else ""
                    if path_text == "TEMPORARY_OUTPUT":
                        tmp_dir = tempfile.mkdtemp(prefix="geoengine-qgis-output-", dir=_PLUGIN_TMP_DIR)
                        os.chmod(tmp_dir, 0o700)
                        path_text = tmp_dir
                        feedback.pushInfo(
                            f"Using temporary output folder for input '{name}': {path_text!r}"
                        )
                    value = path_text
                    # Also record the resolved folder path so _output_dirs_from_parameters
                    # uses the real directory instead of re-reading "TEMPORARY_OUTPUT".
                    resolved_writable_file_paths[name] = path_text
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
        requested_output_dirs = self._output_dirs_from_parameters(
            parameters, context, feedback,
            resolved_writable_files=resolved_writable_file_paths,
        )

        try:
            result = client.run_tool(
                worker=self._worker,
                inputs=inputs,
                on_output=lambda line: feedback.pushInfo(line),
                is_cancelled=lambda: feedback.isCanceled(),
                ver=self._ver,
                dev_mode=self._dev_mode,
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

