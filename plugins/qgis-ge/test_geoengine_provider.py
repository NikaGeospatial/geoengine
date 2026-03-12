"""
Unit tests for GeoEngine QGIS Provider

Tests the core functionality of the QGIS plugin provider without requiring
a full QGIS environment by mocking the dependencies.
"""

import unittest
from unittest.mock import Mock, patch, MagicMock
import json
import os
import tempfile


class TestGeoEngineCLIClient(unittest.TestCase):
    """Test cases for GeoEngineCLIClient"""

    @patch('geoengine_provider.shutil.which')
    def test_find_binary_with_which(self, mock_which):
        """Test finding geoengine binary using which"""
        from geoengine_provider import GeoEngineCLIClient

        mock_which.return_value = '/usr/local/bin/geoengine'

        client = GeoEngineCLIClient()
        self.assertEqual(client.binary, '/usr/local/bin/geoengine')

    @patch('geoengine_provider.shutil.which')
    @patch('geoengine_provider.os.path.isfile')
    @patch('geoengine_provider.os.access')
    def test_find_binary_fallback(self, mock_access, mock_isfile, mock_which):
        """Test finding binary via fallback when which fails"""
        from geoengine_provider import GeoEngineCLIClient

        mock_which.return_value = None
        mock_isfile.return_value = True
        mock_access.return_value = True

        with patch.dict(os.environ, {'HOME': '/home/test'}):
            client = GeoEngineCLIClient()
            # Should find binary in one of the fallback paths
            self.assertIsNotNone(client.binary)

    @patch('geoengine_provider.shutil.which')
    @patch('geoengine_provider.os.path.isfile')
    def test_find_binary_not_found(self, mock_isfile, mock_which):
        """Test exception when binary not found"""
        from geoengine_provider import GeoEngineCLIClient

        mock_which.return_value = None
        mock_isfile.return_value = False

        with self.assertRaises(FileNotFoundError):
            GeoEngineCLIClient()


class TestDevModeSettings(unittest.TestCase):
    """Test cases for dev mode settings"""

    @patch('geoengine_provider.QgsSettings')
    def test_is_dev_mode_enabled_default(self, mock_qgs_settings):
        """Test default dev mode is disabled"""
        from geoengine_provider import is_dev_mode_enabled

        mock_settings = Mock()
        mock_settings.value.return_value = False
        mock_qgs_settings.return_value = mock_settings

        result = is_dev_mode_enabled()
        self.assertFalse(result)

    @patch('geoengine_provider.QgsSettings')
    def test_set_dev_mode_enabled(self, mock_qgs_settings):
        """Test setting dev mode enabled"""
        from geoengine_provider import set_dev_mode_enabled

        mock_settings = Mock()
        mock_qgs_settings.return_value = mock_settings

        set_dev_mode_enabled(True)
        mock_settings.setValue.assert_called_once()
        mock_settings.sync.assert_called_once()


class TestWorkerVersionsPersistence(unittest.TestCase):
    """Test cases for worker version persistence"""

    def test_load_worker_versions_empty(self):
        """Test loading worker versions when file doesn't exist"""
        from geoengine_provider import load_worker_versions

        with patch('geoengine_provider.open', side_effect=FileNotFoundError):
            result = load_worker_versions()
            self.assertEqual(result, {})

    def test_load_worker_versions_valid(self):
        """Test loading valid worker versions"""
        from geoengine_provider import load_worker_versions

        test_data = {
            "worker1": ["latest", "1.0.0"],
            "worker2": ["latest", "2.0.0", "2.0.1"]
        }

        with patch('geoengine_provider.open', unittest.mock.mock_open(read_data=json.dumps(test_data))):
            result = load_worker_versions()
            self.assertEqual(result, test_data)

    def test_load_worker_versions_invalid_format(self):
        """Test loading invalid worker versions returns empty dict"""
        from geoengine_provider import load_worker_versions

        with patch('geoengine_provider.open', unittest.mock.mock_open(read_data='not json')):
            result = load_worker_versions()
            self.assertEqual(result, {})

    def test_save_worker_versions(self):
        """Test saving worker versions"""
        from geoengine_provider import save_worker_versions

        test_data = {"worker1": ["latest"]}

        with patch('geoengine_provider.open', unittest.mock.mock_open()) as mock_file:
            save_worker_versions(test_data)
            mock_file.assert_called_once()


class TestGeoEngineAlgorithm(unittest.TestCase):
    """Test cases for GeoEngineAlgorithm"""

    def setUp(self):
        """Set up test fixtures"""
        self.tool_info = {
            'name': 'test-worker',
            'description': 'Test worker description',
            'version': '1.0.0',
            'inputs': [
                {
                    'name': 'input-file',
                    'param_type': 'file',
                    'required': True,
                    'description': 'Input file',
                    'readonly': True,
                    'filetypes': ['.tif', '.tiff']
                },
                {
                    'name': 'output-folder',
                    'param_type': 'folder',
                    'required': True,
                    'description': 'Output folder',
                    'readonly': False
                }
            ]
        }

    def test_algorithm_name_no_version(self):
        """Test algorithm name without version"""
        from geoengine_provider import GeoEngineAlgorithm

        alg = GeoEngineAlgorithm('test-worker', self.tool_info, ver=None)
        self.assertEqual(alg.name(), 'test-worker')

    def test_algorithm_name_with_version(self):
        """Test algorithm name with version"""
        from geoengine_provider import GeoEngineAlgorithm

        alg = GeoEngineAlgorithm('test-worker', self.tool_info, ver='1.2.3')
        self.assertEqual(alg.name(), 'test-worker__ver__1_2_3')

    def test_display_name_no_version(self):
        """Test display name without version"""
        from geoengine_provider import GeoEngineAlgorithm

        alg = GeoEngineAlgorithm('test-worker', self.tool_info, ver=None)
        self.assertEqual(alg.displayName(), 'test-worker')

    def test_display_name_with_version(self):
        """Test display name with version"""
        from geoengine_provider import GeoEngineAlgorithm

        alg = GeoEngineAlgorithm('test-worker', self.tool_info, ver='1.2.3')
        self.assertEqual(alg.displayName(), 'test-worker (1.2.3)')

    def test_format_age_recent(self):
        """Test formatting recent timestamp"""
        from geoengine_provider import GeoEngineAlgorithm
        from datetime import datetime, timedelta, timezone

        # Create timestamp 30 seconds ago
        timestamp = (datetime.now(timezone.utc) - timedelta(seconds=30)).isoformat()

        result = GeoEngineAlgorithm._format_age(timestamp)
        self.assertIn('30s ago', result)

    def test_format_age_minutes(self):
        """Test formatting timestamp in minutes"""
        from geoengine_provider import GeoEngineAlgorithm
        from datetime import datetime, timedelta, timezone

        # Create timestamp 5 minutes ago
        timestamp = (datetime.now(timezone.utc) - timedelta(minutes=5)).isoformat()

        result = GeoEngineAlgorithm._format_age(timestamp)
        self.assertIn('5min', result)

    def test_format_age_over_hour(self):
        """Test formatting timestamp over an hour"""
        from geoengine_provider import GeoEngineAlgorithm
        from datetime import datetime, timedelta, timezone

        # Create timestamp 2 hours ago
        timestamp = (datetime.now(timezone.utc) - timedelta(hours=2)).isoformat()

        result = GeoEngineAlgorithm._format_age(timestamp)
        self.assertEqual(result, 'over an hour ago')

    def test_strip_qgis_source_uri_suffix(self):
        """Test stripping QGIS source URI suffixes"""
        from geoengine_provider import GeoEngineAlgorithm

        # Test with layername suffix
        uri = '/path/to/file.gpkg|layername=layer1'
        result = GeoEngineAlgorithm._strip_qgis_source_uri_suffix(uri)
        self.assertEqual(result, '/path/to/file.gpkg')

        # Test without suffix
        uri = '/path/to/file.tif'
        result = GeoEngineAlgorithm._strip_qgis_source_uri_suffix(uri)
        self.assertEqual(result, '/path/to/file.tif')

        # Test with file:// protocol
        uri = 'file:///path/to/file.shp'
        result = GeoEngineAlgorithm._strip_qgis_source_uri_suffix(uri)
        self.assertEqual(result, '/path/to/file.shp')

    def test_is_supported_output_file(self):
        """Test checking supported output file types"""
        from geoengine_provider import GeoEngineAlgorithm

        # Supported extensions
        self.assertTrue(GeoEngineAlgorithm._is_supported_output_file('/path/file.tif'))
        self.assertTrue(GeoEngineAlgorithm._is_supported_output_file('/path/file.shp'))
        self.assertTrue(GeoEngineAlgorithm._is_supported_output_file('/path/file.geojson'))

        # Sidecar files should not be supported
        self.assertFalse(GeoEngineAlgorithm._is_supported_output_file('/path/file.aux.xml'))
        self.assertFalse(GeoEngineAlgorithm._is_supported_output_file('/path/file.prj'))
        self.assertFalse(GeoEngineAlgorithm._is_supported_output_file('/path/file.shx'))

        # No extension files should be supported (for probing)
        self.assertTrue(GeoEngineAlgorithm._is_supported_output_file('/path/file'))

    def test_safe_temp_stem(self):
        """Test creating safe temp file stems"""
        from geoengine_provider import GeoEngineAlgorithm

        # Normal name
        result = GeoEngineAlgorithm._safe_temp_stem('my-input')
        self.assertEqual(result, 'my-input')

        # Name with spaces
        result = GeoEngineAlgorithm._safe_temp_stem('my input file')
        self.assertEqual(result, 'my_input_file')

        # Name with special characters
        result = GeoEngineAlgorithm._safe_temp_stem('my@input#file!')
        self.assertEqual(result, 'my_input_file_')

        # Empty name
        result = GeoEngineAlgorithm._safe_temp_stem('')
        self.assertEqual(result, 'input')

    def test_parameter_bool_conversions(self):
        """Test boolean parameter conversions"""
        from geoengine_provider import GeoEngineAlgorithm

        # Boolean values
        self.assertTrue(GeoEngineAlgorithm._parameter_bool({'key': True}, 'key'))
        self.assertFalse(GeoEngineAlgorithm._parameter_bool({'key': False}, 'key'))

        # String values
        self.assertTrue(GeoEngineAlgorithm._parameter_bool({'key': 'true'}, 'key'))
        self.assertTrue(GeoEngineAlgorithm._parameter_bool({'key': '1'}, 'key'))
        self.assertTrue(GeoEngineAlgorithm._parameter_bool({'key': 'yes'}, 'key'))
        self.assertFalse(GeoEngineAlgorithm._parameter_bool({'key': 'false'}, 'key'))
        self.assertFalse(GeoEngineAlgorithm._parameter_bool({'key': '0'}, 'key'))

        # Integer values
        self.assertTrue(GeoEngineAlgorithm._parameter_bool({'key': 1}, 'key'))
        self.assertFalse(GeoEngineAlgorithm._parameter_bool({'key': 0}, 'key'))

        # Missing key
        self.assertFalse(GeoEngineAlgorithm._parameter_bool({}, 'key', default=False))
        self.assertTrue(GeoEngineAlgorithm._parameter_bool({}, 'key', default=True))


class TestWorkerVersionsDialog(unittest.TestCase):
    """Test cases for WorkerVersionsDialog"""

    def setUp(self):
        """Set up test fixtures"""
        self.worker_versions_info = {
            'worker1': ['latest', '1.0.0', '1.0.1'],
            'worker2': ['latest', '2.0.0']
        }
        self.current_selections = {
            'worker1': ['latest'],
            'worker2': ['latest', '2.0.0']
        }

    @patch('geoengine_provider.QDialog')
    def test_dialog_initialization(self, mock_dialog):
        """Test dialog initializes with correct data"""
        from geoengine_provider import WorkerVersionsDialog

        dialog = WorkerVersionsDialog(
            self.worker_versions_info,
            self.current_selections,
            None
        )

        self.assertEqual(dialog._worker_versions_info, self.worker_versions_info)
        self.assertEqual(dialog._current_selections, self.current_selections)


if __name__ == '__main__':
    unittest.main()