from qgis.PyQt.QtWidgets import (
    QWidget, QHBoxLayout, QVBoxLayout,
    QRadioButton, QButtonGroup, QFrame,
    QStackedWidget, QSizePolicy
)
import os
from qgis.PyQt.QtCore import Qt
from qgis.core import (
    QgsProcessingParameterDefinition,
    QgsProcessingParameterMapLayer,
    QgsProcessingParameterFile,
    QgsProcessingContext,
    QgsProcessingParameterType,
    QgsProcessingFeedback,
)
from qgis.gui import (
    QgsAbstractProcessingParameterWidgetWrapper,
    QgsProcessingParameterWidgetContext,
    QgsProcessingGui,
)
import qgis.gui

"""
Custom QGIS Processing Parameter: QgsProcessingParameterLayerOrFile
=======================================================
A parameter that lets the user choose between a map layer or a file input
via a radio button selector. Only the relevant widget is shown at a time.

Usage in your algorithm's initAlgorithm():
    self.addParameter(
        QgsProcessingParameterLayerOrFile(
            name='INPUT',
            description='Input Layer or File',
        )
    )

Then in processAlgorithm(), retrieve the value with:
    value = self.parameterAsLayerOrFile(parameters, 'INPUT', context)
    # Returns either a QgsMapLayer or a file path string.
"""

# ---------------------------------------------------------------------------
# 1. The Parameter Definition
# ---------------------------------------------------------------------------

class QgsProcessingParameterLayerOrFile(QgsProcessingParameterDefinition):
    """
    A custom processing parameter that wraps either a MapLayer or a File
    parameter, selectable via radio buttons in the UI.
    """

    # Custom type string — must be unique across all parameters in QGIS
    TYPE = 'layer_or_file'

    def __init__(
        self,
        name,
        description='',
        default=None,
        optional=False,
        filetypes=None,
    ):
        super().__init__(name, description, default, optional)
        # Keep only concrete extensions; ".*" means wildcard/all files.
        self._filetypes = [
            str(ft).strip() for ft in (filetypes or []) if str(ft).strip() and str(ft).strip() != ".*"
        ]

    def clone(self):
        optional = bool(
            self.flags() & qgis.core.Qgis.ProcessingParameterFlag.Optional
        )
        cloned = QgsProcessingParameterLayerOrFile(
            self.name(),
            self.description(),
            self.defaultValue(),
            optional,
            filetypes=self._filetypes,
        )
        return cloned

    def type(self):
        return self.TYPE

    def checkValueIsAcceptable(self, value, context=None):
        # Accept anything a layer or file parameter would accept
        if value is None or value == "":
            return bool(
                self.flags() & qgis.core.Qgis.ProcessingParameterFlag.Optional
            )
        return True

    def valueAsPythonString(self, value, context):
        if value is None:
            return 'None'
        return repr(value)

    def asScriptCode(self):
        return f"###{self.name()}=layer_or_file"

    def filetypes(self):
        return list(self._filetypes)

# ---------------------------------------------------------------------------
# 2. The Widget (shown inside the Processing dialog)
# ---------------------------------------------------------------------------

class QgsLayerOrFileParameterWidget(QWidget):
    """
    The actual UI widget. Shows radio buttons on the left and the
    appropriate sub-widget (layer or file picker) on the right.
    """

    def __init__(self, param, dialog, row=0, col=0, parent=None):
        super().__init__(parent)
        self._param = param
        self._dialog = dialog
        self._file_filter = self._build_file_filter(param)

        # --- Root layout: radio buttons on left, dynamic widget on right ---
        root_layout = QHBoxLayout(self)
        root_layout.setContentsMargins(0, 0, 0, 0)
        root_layout.setSpacing(6)

        # --- Radio buttons stacked vertically, flushed left ---
        radio_frame = QFrame()
        radio_layout = QVBoxLayout(radio_frame)
        radio_layout.setContentsMargins(0, 0, 0, 0)
        radio_layout.setSpacing(1)

        self._radio_layer = QRadioButton("Layer")
        self._radio_file = QRadioButton("File")
        self._radio_layer.setChecked(True)

        self._button_group = QButtonGroup(self)
        self._button_group.addButton(self._radio_layer)
        self._button_group.addButton(self._radio_file)
        self._button_group.setExclusive(True)

        radio_layout.addWidget(self._radio_layer)
        radio_layout.addWidget(self._radio_file)
        radio_layout.addStretch(1)
        root_layout.addWidget(radio_frame, 0, Qt.AlignTop)  # stretch=0 keeps it tight

        # --- Build inner layer parameter widget ---
        self._layer_param = QgsProcessingParameterMapLayer(
            param.name() + '_layer',
            param.description(),
            optional=bool(param.flags() & qgis.core.Qgis.ProcessingParameterFlag.Optional)
        )
        self._layer_wrapper = self._make_wrapper(self._layer_param)
        self._layer_widget = self._layer_wrapper.createWrappedWidget(
            QgsProcessingContext()
        )

        # --- Build inner file parameter widget ---
        self._file_param = QgsProcessingParameterFile(
            param.name() + '_file',
            param.description(),
            optional=bool(param.flags() & qgis.core.Qgis.ProcessingParameterFlag.Optional)
        )
        if hasattr(self._file_param, 'setFileFilter'):
            self._file_param.setFileFilter(self._file_filter)
        self._file_wrapper = self._make_wrapper(self._file_param)
        self._file_widget = self._file_wrapper.createWrappedWidget(
            QgsProcessingContext()
        )

        # Stack both sub-widgets; only the active one is shown
        self._stack = QStackedWidget()
        self._stack.addWidget(self._layer_widget)  # index 0
        self._stack.addWidget(self._file_widget)   # index 1
        self._stack.setContentsMargins(0, 0, 0, 0)
        max_width = max(self._layer_widget.sizeHint().width(), self._file_widget.sizeHint().width())
        max_height = max(self._layer_widget.sizeHint().height(), self._file_widget.sizeHint().height())
        self._stack.setMinimumSize(max_width, max_height)
        self._stack.setSizePolicy(QSizePolicy.Expanding, QSizePolicy.Fixed)
        self._set_mode(True)
        root_layout.addWidget(self._stack, 1, Qt.AlignTop)

        # Connect radio buttons
        self._radio_layer.toggled.connect(
            lambda checked: self._on_mode_changed(True, checked)
        )
        self._radio_file.toggled.connect(
            lambda checked: self._on_mode_changed(False, checked)
        )

    def _make_wrapper(self, param):
        """Create a standard widget wrapper for a built-in parameter type."""
        registry = qgis.gui.QgsGui.processingGuiRegistry()
        wrapper = registry.createParameterWidgetWrapper(
            param, QgsProcessingGui.Standard
        )
        return wrapper

    @staticmethod
    def _build_file_filter(param):
        filetypes = []
        if hasattr(param, 'filetypes'):
            try:
                filetypes = param.filetypes() or []
            except Exception:
                filetypes = []

        patterns = []
        for filetype in filetypes:
            ext = str(filetype).strip()
            if not ext:
                continue
            if ext.startswith("*."):
                patterns.append(ext)
            elif ext.startswith("."):
                patterns.append(f"*{ext}")
            elif ext == "*":
                patterns.append("*.*")
            else:
                patterns.append(f"*.{ext.lstrip('*.')}")

        if not patterns:
            return "All files (*.*)"

        unique_patterns = " ".join(sorted(set(patterns)))
        return f"Accepted files ({unique_patterns});;All files (*.*)"

    def _set_mode(self, use_layer):
        self._stack.setCurrentIndex(0 if use_layer else 1)

    def _on_mode_changed(self, use_layer, checked):
        """Toggle visibility of the sub-widgets based on radio selection."""
        if checked:
            self._set_mode(use_layer)

    def is_layer_mode(self):
        return self._radio_layer.isChecked()

    def value(self):
        """Return the current value from whichever widget is active."""
        if self.is_layer_mode():
            return self._layer_wrapper.parameterValue()
        else:
            return self._file_wrapper.parameterValue()

    def setValue(self, value):
        """Pre-populate the widget with an existing value."""
        if value is not None:
            # If it looks like a file path, switch to file mode
            if isinstance(value, str) and (
                os.path.isabs(value) or os.path.sep in value or '/' in value or '\\' in value
            ):
                self._radio_file.setChecked(True)
                self._file_wrapper.setParameterValue(value, QgsProcessingContext())
            else:
                self._radio_layer.setChecked(True)
                self._layer_wrapper.setParameterValue(value, QgsProcessingContext())

# ---------------------------------------------------------------------------
# 3. The Widget Wrapper (bridges the widget and the Processing framework)
# ---------------------------------------------------------------------------

class QgsLayerOrFileParameterWidgetWrapper(QgsAbstractProcessingParameterWidgetWrapper):
    """
    The glue between QGIS Processing and our custom widget.
    This is what the Processing dialog actually instantiates.
    """

    def __init__(self, parameter, dialog, row=0, col=0, parent=None):
        super().__init__(parameter, QgsProcessingGui.Standard)
        # Cache the parameter definition now while the C++ object is still
        # alive.  By the time createWidget() is called the underlying C++
        # QgsAbstractProcessingParameterWidgetWrapper may already have been
        # deleted (ownership transferred to C++), which makes
        # self.parameterDefinition() crash with "wrapped C/C++ object …
        # has been deleted".
        self._param_def = parameter
        self._widget = None

    def createWidget(self):
        self._widget = QgsLayerOrFileParameterWidget(
            self._param_def,
            None,
        )
        return self._widget

    def setWidgetValue(self, value, context):
        if self._widget:
            self._widget.setValue(value)

    def widgetValue(self):
        if self._widget:
            return self._widget.value()
        return None

# ---------------------------------------------------------------------------
# 4. The Widget Factory (registers our wrapper with QGIS)
# ---------------------------------------------------------------------------

class QgsLayerOrFileParameterWidgetFactory(qgis.gui.QgsProcessingParameterWidgetFactoryInterface):
    """
    Tells the Processing GUI how to create widgets for LayerOrFileParameter.
    Register this once when your plugin loads.
    """

    def __init__(self):
        super().__init__()
        self._wrappers = []

    def parameterType(self):
        return QgsProcessingParameterLayerOrFile.TYPE

    def createWidgetWrapper(self, parameter, type):
        wrapper = QgsLayerOrFileParameterWidgetWrapper(parameter, None)
        self._wrappers.append(wrapper)
        return wrapper

# ---------------------------------------------------------------------------
# 5. The Parameter Type (registers the parameter with QGIS Processing core)
# ---------------------------------------------------------------------------

class QgsProcessingParameterLayerOrFileType(QgsProcessingParameterType):
    """
    Registers LayerOrFileParameter so QGIS Processing core knows about it.
    Register this once when your plugin loads.
    """

    def create(self, name):
        return QgsProcessingParameterLayerOrFile(name)

    def metadata(self):
        return {
            'name': 'Layer or File',
            'description': 'Choose between a map layer or a file path.',
            'id': QgsProcessingParameterLayerOrFile.TYPE,
        }

    def id(self):
        return QgsProcessingParameterLayerOrFile.TYPE

    def name(self):
        return 'Layer or File'

    def className(self):
        return 'LayerOrFileParameter'
