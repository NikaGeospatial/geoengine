# -*- coding: utf-8 -*-
"""
GeoEngine Tools - ArcGIS Pro Python Toolbox
Invokes the geoengine CLI directly to execute containerized geoprocessing tools.
"""

import arcpy
import os
from geoengine_client import GeoEngineClient


class Toolbox:
    def __init__(self):
        """Define the toolbox (the name of the toolbox is the name of the .pyt file)."""
        self.label = "GeoEngine Tools"
        self.alias = "geoengine"

        # Discover tools from GeoEngine CLI
        self.tools = self._discover_tools()

    def _discover_tools(self):
        """Discover available tools from the geoengine CLI."""
        try:
            client = GeoEngineClient()
            workers = client.list_workers()

            tools = []
            for worker in workers:
                if not worker.get('has_tool', False):
                    continue
                tool_info = client.get_worker_tool(worker['name'])
                if tool_info:
                    tool_class = self._create_tool_class(worker['name'], tool_info)
                    tools.append(tool_class)

            return tools if tools else [GeoEngineStatusTool]
        except Exception as e:
            arcpy.AddWarning(f"Could not discover GeoEngine tools: {e}")
            return [GeoEngineStatusTool]

    def _create_tool_class(self, worker_name, tool_info):
        """Create a dynamic tool class for a GeoEngine worker."""

        class DynamicTool:
            def __init__(self):
                self.label = tool_info.get('name', worker_name)
                self.description = tool_info.get('description', '')
                self.category = 'GeoEngine'
                self.canRunInBackground = True
                self._worker = worker_name
                self._inputs = tool_info.get('inputs', [])

            def getParameterInfo(self):
                """Define parameter definitions."""
                params = []
                for inp in self._inputs:
                    param = self._create_parameter(inp)
                    params.append(param)
                return params

            def _create_parameter(self, param_info):
                """Create an arcpy parameter from worker input parameter info."""
                param_type = param_info.get('param_type', 'string')

                type_map = {
                    'file': 'DEFile',
                    'folder': 'DEFolder',
                    'datetime': 'GPString',
                    'string': 'GPString',
                    'number': 'GPDouble',
                    'boolean': 'GPBoolean',
                    'enum': 'GPString',
                }

                arcpy_type = type_map.get(param_type, 'GPString')
                required = param_info.get('required', True)

                param = arcpy.Parameter(
                    displayName=param_info.get('description', param_info['name']),
                    name=param_info['name'],
                    datatype=arcpy_type,
                    parameterType='Required' if required else 'Optional',
                    direction='Input',
                )

                if 'default' in param_info and param_info['default'] is not None:
                    param.value = param_info['default']

                # For enum type, set the filter list
                if param_type == 'enum':
                    enum_values = param_info.get('enum_values', [])
                    if enum_values:
                        param.filter.type = "ValueList"
                        param.filter.list = enum_values

                return param

            def isLicensed(self):
                return True

            def updateParameters(self, parameters):
                return

            def updateMessages(self, parameters):
                return

            def execute(self, parameters, messages):
                """Execute the tool via geoengine CLI."""
                try:
                    client = GeoEngineClient()

                    inputs = {}
                    for param in parameters:
                        if param.value is not None:
                            if hasattr(param.value, 'dataSource'):
                                inputs[param.name] = param.value.dataSource
                            else:
                                inputs[param.name] = str(param.value)

                    messages.addMessage(f"Running worker '{self._worker}'...")

                    result = client.run_tool(
                        worker=self._worker,
                        inputs=inputs,
                        on_output=lambda line: messages.addMessage(line),
                    )

                    messages.addMessage("Worker completed successfully!")
                    output_files = result.get('files', [])
                    if output_files:
                        messages.addMessage(f"Output files: {len(output_files)}")
                        for f in output_files:
                            messages.addMessage(f"  {f['name']} ({f.get('size', 0)} bytes)")

                except Exception as e:
                    error_message = f"Error executing tool: {e}"
                    messages.addErrorMessage(error_message)
                    raise arcpy.ExecuteError(error_message) from e

            def postExecute(self, parameters):
                return

        DynamicTool.__name__ = f"geoengine_{worker_name}"
        return DynamicTool


class GeoEngineStatusTool:
    """Tool to check GeoEngine CLI status."""

    def __init__(self):
        self.label = "Check GeoEngine Status"
        self.description = "Check that the geoengine CLI binary is available and list registered workers"
        self.canRunInBackground = False

    def getParameterInfo(self):
        return []

    def isLicensed(self):
        return True

    def updateParameters(self, parameters):
        return

    def updateMessages(self, parameters):
        return

    def execute(self, parameters, messages):
        try:
            client = GeoEngineClient()
            info = client.version_check()

            messages.addMessage(f"GeoEngine: {info['version']}")
            messages.addMessage(f"Status: {info['status']}")

            workers = client.list_workers()
            if workers:
                messages.addMessage(f"\nRegistered Workers ({len(workers)}):")
                for w in workers:
                    has_tool = "yes" if w.get('has_tool', False) else "no"
                    messages.addMessage(f"  - {w['name']} (tool: {has_tool})")
            else:
                messages.addMessage("\nNo workers registered.")

        except FileNotFoundError as e:
            messages.addErrorMessage(str(e))
            messages.addMessage("\nInstall geoengine and ensure it is on your PATH:")
            messages.addMessage(
                "  https://github.com/NikaGeospatial/geoengine"
            )
        except Exception as e:
            messages.addErrorMessage(f"Error: {e}")

    def postExecute(self, parameters):
        return
