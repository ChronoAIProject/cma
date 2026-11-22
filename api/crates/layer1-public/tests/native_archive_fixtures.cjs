const { crc32 } = require("node:zlib");

// Use uncompressed entries so the minimal fixtures have deterministic bytes.
function storedZip(entries) {
  const localParts = [];
  const directoryParts = [];
  let offset = 0;
  for (const [name, content] of Object.entries(entries)) {
    const path = Buffer.from(name);
    const bytes = Buffer.from(content);
    const local = Buffer.alloc(30);
    const directory = Buffer.alloc(46);
    local.writeUInt32LE(0x04034b50);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(33, 12);
    local.writeUInt32LE(crc32(bytes), 14);
    local.writeUInt32LE(bytes.length, 18);
    local.writeUInt32LE(bytes.length, 22);
    local.writeUInt16LE(path.length, 26);
    directory.writeUInt32LE(0x02014b50);
    directory.writeUInt16LE(20, 4);
    local.copy(directory, 6, 4, 28);
    directory.writeUInt32LE(offset, 42);
    localParts.push(local, path, bytes);
    directoryParts.push(directory, path);
    offset += local.length + path.length + bytes.length;
  }
  const centralDirectory = Buffer.concat(directoryParts);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50);
  end.writeUInt16LE(directoryParts.length / 2, 8);
  end.writeUInt16LE(directoryParts.length / 2, 10);
  end.writeUInt32LE(centralDirectory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...localParts, centralDirectory, end]);
}

const packageNamespace = "http://schemas.openxmlformats.org/package/2006";
const officeRelationships = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const wordRoot = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const sheetRoot = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const officeType = "application/vnd.openxmlformats-officedocument";

function relationships(content) {
  return `<Relationships xmlns="${packageNamespace}/relationships">${content}</Relationships>`;
}

function officePackage(root, files) {
  const types = Object.entries(files).map(([path, [type]]) =>
    `<Override PartName="/${path}" ContentType="${type}"/>`
  );
  return storedZip({
    "[Content_Types].xml":
      `<Types xmlns="${packageNamespace}/content-types">` +
      `<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>` +
      `${types.join("")}</Types>`,
    "_rels/.rels": relationships(
      `<Relationship Id="main" Type="${officeRelationships}/officeDocument" Target="${root}"/>`
    ),
    ...Object.fromEntries(Object.entries(files).map(([path, [, content]]) => [path, content])),
  });
}

const word = officePackage("word/document.xml", {
  "word/document.xml": [
    `${officeType}.wordprocessingml.document.main+xml`,
    `<w:document xmlns:w="${wordRoot}"><w:body><w:p><w:r><w:t>Native test report</w:t></w:r></w:p></w:body></w:document>`,
  ],
});

const workbook = officePackage("xl/workbook.xml", {
  "xl/workbook.xml": [
    `${officeType}.spreadsheetml.sheet.main+xml`,
    `<workbook xmlns="${sheetRoot}" xmlns:r="${officeRelationships}"><sheets><sheet name="Results" sheetId="1" r:id="sheet"/></sheets></workbook>`,
  ],
  "xl/_rels/workbook.xml.rels": [
    "application/vnd.openxmlformats-package.relationships+xml",
    relationships(`<Relationship Id="sheet" Type="${officeRelationships}/worksheet" Target="worksheets/sheet1.xml"/>`),
  ],
  "xl/worksheets/sheet1.xml": [
    `${officeType}.spreadsheetml.worksheet+xml`,
    `<worksheet xmlns="${sheetRoot}"><sheetData><row r="1"><c r="A1"><v>3</v></c></row></sheetData></worksheet>`,
  ],
});

module.exports = { word, workbook, archive: storedZip({ "result.txt": "Result: 3\n" }) };
