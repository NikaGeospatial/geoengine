# -*- coding: utf-8 -*-
"""
GeoEngine QGIS Plugin - Main plugin class
"""

import os

from qgis.core import QgsApplication
from qgis.PyQt.QtCore import QFileSystemWatcher
from qgis.PyQt.QtWidgets import QAction, QMessageBox
from qgis.PyQt.QtGui import QIcon

from .geoengine_provider import (
    GeoEngineProvider,
    GeoEngineCLIClient,
    is_dev_mode_enabled,
    set_dev_mode_enabled,
)

# Sentinel file that geoengine apply touches to signal a refresh
_REFRESH_TRIGGER = os.path.join(
    os.path.expanduser('~'), '.geoengine', '.qgis_refresh'
)


class GeoEnginePlugin:
    """QGIS Plugin Implementation for GeoEngine."""

    def __init__(self, iface):
        """Constructor.

        Args:
            iface: An interface instance that will be passed to this class
                which provides the hook by which you can manipulate the QGIS
                application at run time.
        """
        self.iface = iface
        self.provider = None
        self.actions = []
        self.menu = '&GeoEngine'
        self._watcher = None

    def initProcessing(self):
        """Initialize the processing provider."""
        self.provider = GeoEngineProvider()
        QgsApplication.processingRegistry().addProvider(self.provider)

    def _setup_file_watcher(self):
        """Watch the refresh trigger file so apply can push updates."""
        # Ensure the trigger file exists so the watcher has something to track
        trigger_dir = os.path.dirname(_REFRESH_TRIGGER)
        os.makedirs(trigger_dir, exist_ok=True)
        if not os.path.exists(_REFRESH_TRIGGER):
            with open(_REFRESH_TRIGGER, 'w') as f:
                f.write('')

        self._watcher = QFileSystemWatcher()
        self._watcher.addPath(_REFRESH_TRIGGER)
        self._watcher.fileChanged.connect(self._on_trigger_changed)

    def _on_trigger_changed(self, path):
        """Called when the trigger file is modified by geoengine apply."""
        self._do_silent_refresh()
        # Re-watch: some OS's drop the watch after a write
        if path not in self._watcher.files():
            if os.path.exists(path):
                self._watcher.addPath(path)

    def _do_silent_refresh(self):
        """Reload algorithms without any dialog popups."""
        if self.provider:
            QgsApplication.processingRegistry().removeProvider(self.provider)
            self.provider = GeoEngineProvider()
            QgsApplication.processingRegistry().addProvider(self.provider)

    def initGui(self):
        """Create the menu entries and toolbar icons inside the QGIS GUI."""
        # Initialize processing provider
        self.initProcessing()

        # Start watching for external refresh triggers
        self._setup_file_watcher()

        # Add menu action to check service status
        self.add_action(
            'geoengine_status',
            'Check GeoEngine Status',
            self.show_status,
            menu=self.menu,
        )

        dev_mode_action = QAction('Developer Mode (use dev worker images)', self.iface.mainWindow())
        dev_mode_action.setCheckable(True)
        dev_mode_action.setChecked(is_dev_mode_enabled())
        dev_mode_action.triggered.connect(self.toggle_dev_mode)
        self.iface.addPluginToMenu(self.menu, dev_mode_action)
        self.actions.append(dev_mode_action)

    def add_action(
        self,
        icon_name,
        text,
        callback,
        enabled=True,
        menu=None,
        toolbar=None,
    ):
        """Add a toolbar icon and menu item."""
        action = QAction(text, self.iface.mainWindow())
        action.triggered.connect(callback)
        action.setEnabled(enabled)

        if menu:
            self.iface.addPluginToMenu(menu, action)

        if toolbar:
            toolbar.addAction(action)

        self.actions.append(action)
        return action

    def unload(self):
        """Remove the plugin menu items and icons from QGIS GUI."""
        # Stop watching
        if self._watcher:
            self._watcher.fileChanged.disconnect(self._on_trigger_changed)
            self._watcher = None

        # Remove menu items
        for action in self.actions:
            self.iface.removePluginMenu(self.menu, action)

        # Remove provider
        if self.provider:
            QgsApplication.processingRegistry().removeProvider(self.provider)

    def show_status(self):
        """Show GeoEngine CLI status."""
        try:
            client = GeoEngineCLIClient()
            info = client.version_check()
            workers = client.list_workers()

            msg = f"GeoEngine: {info['version']}\n"
            msg += f"Status: {info['status']}\n\n"
            msg += f"Registered Workers: {len(workers)}\n"

            for w in workers:
                has_tool = "yes" if w.get('has_tool', False) else "no"
                msg += f"  - {w['name']} (tool: {has_tool})\n"

            QMessageBox.information(
                self.iface.mainWindow(),
                "GeoEngine Status",
                msg
            )

        except FileNotFoundError as e:
            QMessageBox.warning(
                self.iface.mainWindow(),
                "GeoEngine Error",
                f"{e}\n\n"
                "Install geoengine and ensure it is on your PATH:\n"
                "  https://github.com/NikaGeospatial/geoengine"
            )
        except Exception as e:
            QMessageBox.warning(
                self.iface.mainWindow(),
                "GeoEngine Error",
                f"Error communicating with geoengine:\n{e}"
            )

    def toggle_dev_mode(self, enabled):
        """Toggle worker execution mode between release and dev images."""
        set_dev_mode_enabled(bool(enabled))
        mode = "enabled" if enabled else "disabled"
        self.iface.messageBar().pushInfo("GeoEngine", f"Dev mode {mode}")
