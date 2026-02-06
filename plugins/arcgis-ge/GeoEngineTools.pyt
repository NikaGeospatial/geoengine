# -*- coding: utf-8 -*-
"""
GeoEngine Tools - ArcGIS Pro Python Toolbox
Invokes the geoengine CLI directly to execute containerized geoprocessing tools.
"""

import arcpy
import os
import time
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
            projects = client.list_projects()

            tools = []
            for project in projects:
                project_tools = client.get_project_tools(project['name'])
                for tool_info in project_tools:
                    # Create a dynamic tool class for each tool
                    tool_class = self._create_tool_class(project['name'], tool_info)
                    tools.append(tool_class)

            return tools if tools else [GeoEngineStatusTool]
        except Exception as e:
            arcpy.AddWarning(f"Could not discover GeoEngine tools: {e}")
            return [GeoEngineStatusTool]

    def _create_tool_class(self, project_name, tool_info):
        """Create a dynamic tool class for a GeoEngine tool."""

        class DynamicTool:
            def __init__(self):
                self.label = tool_info.get('label', tool_info['name'])
                self.description = tool_info.get('description', '')
                self.category = project_name
                self.canRunInBackground = True
                self._project = project_name
                self._tool_name = tool_info['name']
                self._inputs = tool_info.get('inputs', [])
                self._outputs = tool_info.get('outputs', [])

            def getParameterInfo(self):
                """Define parameter definitions."""
                params = []

                # Input parameters
                for i, inp in enumerate(self._inputs):
                    param = self._create_parameter(inp, 'Input')
                    params.append(param)

                # Output parameters
                for i, out in enumerate(self._outputs):
                    param = self._create_parameter(out, 'Output')
                    param.direction = 'Output'
                    params.append(param)

                return params

            def _create_parameter(self, param_info, direction):
                """Create an arcpy parameter from tool parameter info."""
                param_type = param_info.get('param_type', 'string')

                # Map GeoEngine types to ArcGIS types
                type_map = {
                    'raster': 'GPRasterLayer',
                    'vector': 'GPFeatureLayer',
                    'string': 'GPString',
                    'int': 'GPLong',
                    'float': 'GPDouble',
                    'bool': 'GPBoolean',
                    'folder': 'DEFolder',
                    'file': 'DEFile',
                }

                arcpy_type = type_map.get(param_type, 'GPString')
                required = param_info.get('required', True)

                param = arcpy.Parameter(
                    displayName=param_info.get('label', param_info['name']),
                    name=param_info['name'],
                    datatype=arcpy_type,
                    parameterType='Required' if required else 'Optional',
                    direction=direction,
                )

                # Set default value if provided
                if 'default' in param_info and param_info['default'] is not None:
                    param.value = param_info['default']

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

                    # Build inputs dict
                    inputs = {}
                    output_dir = None

                    for param in parameters:
                        if param.direction == 'Input' and param.value is not None:
                            # Convert to string path if it's a dataset
                            if hasattr(param.value, 'dataSource'):
                                inputs[param.name] = param.value.dataSource
                            else:
                                inputs[param.name] = str(param.value)
                        elif param.direction == 'Output':
                            # Use output parameter location as output directory
                            if param.value:
                                output_path = str(param.value)
                                output_dir = os.path.dirname(output_path)

                    messages.addMessage(f"Running tool '{self._tool_name}'...")

                    result = client.run_tool(
                        project=self._project,
                        tool=self._tool_name,
                        inputs=inputs,
                        output_dir=output_dir,
                        on_output=lambda line: messages.addMessage(line),
                    )

                    messages.addMessage("Tool completed successfully!")
                    output_files = result.get('files', [])
                    if output_files:
                        messages.addMessage(f"Output files: {len(output_files)}")
                        for f in output_files:
                            messages.addMessage(f"  {f['name']} ({f.get('size', 0)} bytes)")

                except Exception as e:
                    messages.addErrorMessage(f"Error executing tool: {e}")

            def postExecute(self, parameters):
                return

        # Set a unique class name
        DynamicTool.__name__ = f"{project_name}_{tool_info['name']}"
        return DynamicTool


class GeoEngineStatusTool:
    """Tool to check GeoEngine CLI status."""

    def __init__(self):
        self.label = "Check GeoEngine Status"
        self.description = "Check that the geoengine CLI binary is available and list registered projects"
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

            # List projects
            projects = client.list_projects()
            if projects:
                messages.addMessage(f"\nRegistered Projects ({len(projects)}):")
                for p in projects:
                    messages.addMessage(
                        f"  - {p['name']} ({p.get('tools_count', 0)} tools)"
                    )
            else:
                messages.addMessage("\nNo projects registered.")

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
