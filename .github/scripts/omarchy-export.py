"""Retain an experimental candidate with its explicit admission result."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys

source = Path(__file__).with_name('export-ci-native-image.py')
spec = importlib.util.spec_from_file_location('image_export', source)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
revision, destination, evidence = sys.argv[1:]
destination, evidence = Path(destination), Path(evidence)
module.export('runtime-omarchy', revision, destination)
path = destination / 'metadata.json'
value = json.loads(path.read_text())
desktop, scan = os.environ['DESKTOP_OUTCOME'], os.environ['SCAN_OUTCOME']
value['schema'] = 'cma-experimental-image-v1'
contract, graphics = os.environ['CONTRACT_OUTCOME'], os.environ['GRAPHICS_OUTCOME']
image_tmp = os.environ['IMAGE_TMP_OUTCOME']
value['verification'] = {'desktop': desktop, 'virtualDrmDesktop': graphics,
                         'imageTemporaryDirectory': image_tmp,
                         'runtimeContract': contract, 'vulnerabilityScan': scan}
value['admission'] = 'local-prototype-only' if desktop == scan == contract == graphics == image_tmp == 'success' else 'blocked'
value['productionValidated'] = False
path.write_text(json.dumps(value, indent=2) + '\n')
shutil.copytree(evidence, destination / 'evidence')
