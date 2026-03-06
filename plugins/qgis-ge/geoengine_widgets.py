import traceback

from qgis.core import QgsMessageLog, QgsProcessingParameterDefinition
from qgis.gui import (
    QgsAbstractProcessingParameterWidgetWrapper,
    QgsProcessingGui,
    QgsMapLayerComboBox,
    QgsProcessingParameterWidgetFactoryInterface
)
from qgis.PyQt.QtCore import Qt, QUrl
from qgis.PyQt.QtWidgets import (
    QWidget, QHBoxLayout, QPushButton, QFileDialog
)


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


class MapLayerWithFileWrapper(QgsAbstractProcessingParameterWidgetWrapper):

    def __init__(self, parameter, dialog_type=QgsProcessingGui.Standard, row=0, col=0, **kwargs):
        super().__init__(parameter, dialog_type)

    def createWidget(self):
        # Outer container
        self._widget = QWidget()
        layout = QHBoxLayout()
        layout.setContentsMargins(0, 0, 0, 0)
        self._widget.setLayout(layout)

        # Left: map layer combo box
        self._combo = QgsMapLayerComboBox()
        is_optional = bool(
            self.parameterDefinition().flags() & QgsProcessingParameterDefinition.FlagOptional
        )
        self._combo.setAllowEmptyLayer(is_optional)
        self._combo.layerChanged.connect(self._on_layer_changed)
        layout.addWidget(self._combo, stretch=1, alignment=Qt.AlignmentFlag.AlignTop)

        # Right: file picker button
        self._button = QPushButton("…")
        self._button.setFixedWidth(23)
        self._button.setFixedHeight(18)
        self._button.setStyleSheet("font-size: 10px;")
        self._button.setToolTip("Select a file")
        self._button.clicked.connect(self._pick_file)
        layout.addWidget(self._button, alignment=Qt.AlignmentFlag.AlignTop)

        self._file_path = ""
        self._use_file = False

        # Build the file-dialog filter from the filetypes declared in metadata.
        # e.g. ['.txt', '.csv'] → "Accepted files (*.txt *.csv);;All Files (*)"
        # An empty list means no restriction → "All Files (*)"
        filetypes = []
        try:
            meta = self.parameterDefinition().metadata()
            filetypes = meta.get('widget_wrapper', {}).get('filetypes', []) or []
        except Exception as e:
            QgsMessageLog.logMessage(
                f"GeoEngine: could not read filetypes from parameterDefinition().metadata(),"
                f" falling back to accept all files: {e}\n{traceback.format_exc()}",
                "GeoEngine",
                0,
            )
        if filetypes:
            pattern = " ".join(f"*{ft}" for ft in filetypes)
            self._file_filter = f"Accepted files ({pattern});;All Files (*)"
        else:
            self._file_filter = "All Files (*)"

        return self._widget

    def _on_layer_changed(self, layer):
        # User picked a real layer — clear any file selection.
        # Ignore None signals: those fire when a file is added as an
        # additional item and selected, which should not reset _file_path.
        if layer is not None:
            self._file_path = ""
            self._use_file = False

    def _pick_file(self):
        # Use the button (the click sender) as parent — self._widget may have
        # been deleted by C++ (ownership transferred to the Processing dialog).
        try:
            import sip
            parent = self._button if not sip.isdeleted(self._button) else None
        except Exception:
            parent = None
        path, _ = QFileDialog.getOpenFileName(
            parent,
            "Select File",
            "",
            self._file_filter
        )
        if path:
            self._file_path = path
            self._use_file = True
            self._reflect_file_in_combo(path)

    def _reflect_file_in_combo(self, path):
        """
        Show the selected file's name in the combo box without loading
        it as a map layer.  Uses setAdditionalItems to append the
        filename as a plain-text entry, then selects it.
        """
        items = self._combo.additionalItems()
        if path not in items:
            items.append(path)
            self._combo.setAdditionalItems(items)
        idx = self._combo.findText(path)
        if idx >= 0:
            self._combo.setCurrentIndex(idx)

    def setWidgetValue(self, value, context):
        if value:
            self._file_path = str(value)
            self._use_file = True
            self._reflect_file_in_combo(self._file_path)

    def widgetValue(self):
        if self._use_file and self._file_path:
            return self._file_path
        layer = self._combo.currentLayer()
        if layer:
            return _strip_qgis_source_uri_suffix(layer.source())
        return ""

    def postInitialize(self, wrappers):
        pass

class MapLayerWithFileFactory(QgsProcessingParameterWidgetFactoryInterface):
    def createWidgetWrapper(self, param, dialog_type):
        return MapLayerWithFileWrapper(param, dialog_type)

    def parameterType(self):
        return "map_layer_with_file"