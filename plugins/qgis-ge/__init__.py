# -*- coding: utf-8 -*-
"""
GeoEngine QGIS Plugin
Integrates GeoEngine containerized geoprocessing tools into QGIS Processing framework.
"""


def classFactory(iface):
    """Load GeoEnginePlugin class from geoengine_plugin module.

    Args:
        iface: A QGIS interface instance.

    Returns:
        GeoEnginePlugin instance.
    """
    from .geoengine_plugin import GeoEnginePlugin
    return GeoEnginePlugin(iface)
