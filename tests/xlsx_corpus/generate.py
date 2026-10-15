#!/usr/bin/env python3
"""Deterministic, authored SpreadsheetML probes; no downloads or Excel dependency."""
import hashlib
import json
from pathlib import Path
import zipfile
import struct
import zlib
from xml.sax.saxutils import escape

HERE = Path(__file__).resolve().parent
NS = 'http://schemas.openxmlformats.org/spreadsheetml/2006/main'
REL = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
PKG = 'http://schemas.openxmlformats.org/package/2006/relationships'

def generate():
    fixtures = []
    for name, purpose, body, extra, names in cases():
        parts = {
            '[Content_Types].xml': '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>',
            '_rels/.rels': f'<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>',
            'xl/workbook.xml': f'<workbook xmlns="{NS}" xmlns:r="{REL}"><sheets><sheet name="Probe" sheetId="1" r:id="rId1"/></sheets>{names}</workbook>',
            'xl/_rels/workbook.xml.rels': f'<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{REL}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="{REL}/styles" Target="styles.xml"/></Relationships>',
            'xl/styles.xml': styles(),
            'xl/worksheets/sheet1.xml': f'<worksheet xmlns="{NS}" xmlns:r="{REL}"><sheetFormatPr baseColWidth="8" defaultRowHeight="15"/><cols><col min="1" max="1" width="24" customWidth="1"/><col min="2" max="2" width="24" customWidth="1"/></cols><sheetData>{body}</sheetData><pageMargins left="0.5" right="0.5" top="0.5" bottom="0.5" header="0.2" footer="0.2"/><pageSetup paperSize="1"/>{extra}</worksheet>',
        }
        if name == 'authored-conditional-styles':
            parts['xl/styles.xml'] = parts['xl/styles.xml'].replace('</styleSheet>', '<dxfs count="3"><dxf><font><sz val="18"/></font><border><left style="thick"><color rgb="FFFF0000"/></left><right style="thick"><color rgb="FFFF0000"/></right><top style="thick"><color rgb="FFFF0000"/></top><bottom style="thick"><color rgb="FFFF0000"/></bottom></border><numFmt numFmtId="10" formatCode="0.00%"/></dxf><dxf><numFmt numFmtId="170" formatCode="yyyy-mm-dd"/></dxf><dxf><numFmt numFmtId="171" formatCode="&quot;$&quot;0.00"/></dxf></dxfs></styleSheet>')
        if name == 'authored-rich-shrink':
            parts['xl/styles.xml'] = parts['xl/styles.xml'].replace('wrapText="1"', 'shrinkToFit="1"')
        if name == 'authored-rich-shared':
            parts['xl/sharedStrings.xml'] = f'<sst xmlns="{NS}" count="1" uniqueCount="1"><si><r><rPr><b/><sz val="20"/><color rgb="FFFF0000"/></rPr><t xml:space="preserve">A large shared </t></r><r><rPr><i/></rPr><t>string wraps across styled runs and keeps TAIL</t></r></si></sst>'
            parts['xl/_rels/workbook.xml.rels'] = parts['xl/_rels/workbook.xml.rels'].replace('</Relationships>', f'<Relationship Id="strings" Type="{REL}/sharedStrings" Target="sharedStrings.xml"/></Relationships>')
            parts['[Content_Types].xml'] = parts['[Content_Types].xml'].replace('</Types>', '<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>')
        if name in ('authored-stacked-chart', 'authored-cropped-image'):
            parts.update(drawing_parts(name))
            parts['[Content_Types].xml'] = parts['[Content_Types].xml'].replace('</Types>', '<Default Extension="png" ContentType="image/png"/><Override PartName="/xl/drawings/drawing1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/><Override PartName="/xl/charts/chart1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/></Types>')
        path = HERE / 'files' / (name + '.xlsx')
        path.parent.mkdir(exist_ok=True)
        with zipfile.ZipFile(path, 'w', compression=zipfile.ZIP_DEFLATED) as z:
            for key, value in sorted(parts.items()):
                info = zipfile.ZipInfo(key, date_time=(2020, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                z.writestr(info, value)
        data = path.read_bytes()
        fixtures.append(dict(id=name, file=path.name, path='tests/xlsx_corpus/files/'+path.name,
                             purpose=purpose, size=len(data), sha256=hashlib.sha256(data).hexdigest()))
    manifest = dict(repository='https://github.com/mnutt/libreoffice-rs',
                    revision='e45b651b4541ce4388f18df5e7313b60e27855a1',
                    generated_by='generate.py', generator_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                    fixtures=fixtures, notices=[])
    (HERE / 'generated.json').write_text(json.dumps(manifest, indent=2)+'\n')

def styles():
    fonts = ''.join(f'<font><sz val="12"/><name val="{family}"/>{variant}</font>' for family, variant in [
        ('Liberation Sans',''), ('Liberation Sans','<b/>'), ('Liberation Sans','<i/>'),
        ('Liberation Serif','<b/><i/>'), ('Liberation Mono','<b/>')])
    xfs = ''.join(f'<xf numFmtId="0" fontId="{i}" fillId="0" borderId="0" xfId="0" applyFont="1" applyAlignment="1"><alignment wrapText="1"/></xf>' for i in range(5))
    return f'<styleSheet xmlns="{NS}"><fonts count="5">{fonts}</fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="5">{xfs}</cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>'

def cell(address, text, style=0):
    return f'<c r="{address}" s="{style}" t="inlineStr"><is><t xml:space="preserve">{escape(text)}</t></is></c>'

def cases():
    rows = ''.join(f'<row r="{r}" ht="35" customHeight="1"><c r="A{r}"><v>{v}</v></c>{cell("B"+str(r), label)}</row>' for r,v,label in [(1,0.125,'Conditional percent and border'),(2,45292,'Conditional date'),(3,12.5,'Conditional currency')])
    rules = ''.join(f'<conditionalFormatting sqref="A{r}"><cfRule type="cellIs" priority="{r}" dxfId="{r-1}" operator="greaterThan"><formula>0</formula></cfRule></conditionalFormatting>' for r in range(1,4))
    yield 'authored-conditional-styles', 'conditional percent/date/currency formats and red borders', rows, rules, ''
    rows = ''.join(f'<row r="{r}" ht="25" customHeight="1"><c r="A{r}"><v>{v}</v></c>{cell("B"+str(r), label)}</row>' for r,v,label in [(1,7,'Single positive'),(2,7,'Repeated positive'),(3,7,'Repeated positive'),(4,0,'Single zero'),(5,-7,'Single negative'),(6,7,'Custom lengths')])
    rules = ''.join(f'<conditionalFormatting sqref="{ref}"><cfRule type="dataBar" priority="{i}"><dataBar {attrs}><cfvo type="min"/><cfvo type="max"/><color rgb="FF4472C4"/></dataBar></cfRule></conditionalFormatting>' for i,(ref,attrs) in enumerate([('A1',''),('A2:A3',''),('A4',''),('A5',''),('A6','minLength="20" maxLength="80"')],1))
    yield 'authored-constant-bars', 'constant and single-cell automatic bars, including zero and negative', rows, rules, ''
    rows = ''.join(f'<row r="{i+1}">{cell("A"+str(i+1), "Wide WWW narrow iii café € nonbreaking\u00a0space", i)}{cell("B"+str(i+1), "Variant "+str(i), i)}</row>' for i in range(5))
    yield 'authored-font-variants', 'real font variants, Unicode widths and automatic wrapping', rows, '', ''
    rows = ''.join(f'<row r="{r}">{cell("A"+str(r), "INCLUDED" if 2 <= r <= 4 else "OUTSIDE")}</row>' for r in range(1,7))
    yield 'authored-print-area', 'print area excludes outside cells', rows, '', '<definedNames><definedName name="_xlnm.Print_Area" localSheetId="0">Probe!$A$2:$A$4</definedName></definedNames>'
    rows = ''.join(f'<row r="{r}">{cell("A"+str(r), "Row "+str(r))}</row>' for r in range(1,11))
    yield 'authored-manual-break', 'explicit horizontal page break after row five', rows, '<rowBreaks count="1" manualBreakCount="1"><brk id="5" min="0" max="16383" man="1"/></rowBreaks>', ''
    runs = '<r><rPr><b/><color rgb="FFFF0000"/></rPr><t>Bold red </t></r><r><rPr><i/><sz val="18"/></rPr><t>large italic</t></r>'
    yield 'authored-rich-text', 'mixed rich-text styles within one cell', f'<row r="1" ht="70" customHeight="1"><c r="A1" t="inlineStr"><is>{runs}</is></c></row>', '', ''

    shrink_runs = '<r><rPr><b/><sz val="20"/><color rgb="FFFF0000"/></rPr><t xml:space="preserve">A wide heading needs </t></r><r><rPr><i/></rPr><t xml:space="preserve">smaller styled text and </t></r><r><rPr><b/><sz val="24"/><color rgb="FF0000FF"/></rPr><t>TAIL</t></r>'
    yield 'authored-rich-shrink', 'shrink mixed-size rich text uniformly; blue tail must remain visible', f'<row r="1" ht="30" customHeight="1"><c r="A1" t="inlineStr"><is>{shrink_runs}</is></c>{cell("B1", "Neighbor")}</row>', '', ''
    yield 'authored-rich-shared', 'shared rich text, mixed-size wrapping and automatic row height', '<row r="1"><c r="A1" t="s"><v>0</v></c>'+cell('B1','Neighbor')+'</row><row r="2">'+cell('A2','NEXT ROW')+'</row>', '', ''
    yield 'authored-stacked-chart', 'stacked columns must accumulate rather than cluster', '<row r="1">'+cell('A1','Stacked chart probe')+'</row><row r="2">'+cell('A2','Total')+'<c r="B2"><v>1</v></c></row><row r="3"><c r="B3"><v>2</v></c></row>', '<drawing r:id="drawing"/>', ''
    yield 'authored-cropped-image', 'image source crop removes left red half', '<row r="1">'+cell('A1','Crop probe: only blue should appear')+'</row>', '<drawing r:id="drawing"/>', ''

def drawing_parts(name):
    a = 'http://schemas.openxmlformats.org/drawingml/2006/main'
    xdr = 'http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing'
    chartns = 'http://schemas.openxmlformats.org/drawingml/2006/chart'
    if name == 'authored-stacked-chart':
        node = f'<xdr:graphicFrame><xdr:nvGraphicFramePr><xdr:cNvPr id="1" name="Stacked"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="{chartns}"><c:chart xmlns:c="{chartns}" r:id="object"/></a:graphicData></a:graphic></xdr:graphicFrame>'
        series = ''.join(f'<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:tx><c:v>Series {i+1}</c:v></c:tx><c:spPr><a:solidFill><a:srgbClr val="{("4472C4","ED7D31")[i]}"/></a:solidFill></c:spPr><c:cat><c:strRef><c:f>Probe!$A$2</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>Total</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:f>Probe!$B${i+2}</c:f><c:numCache><c:formatCode>0</c:formatCode><c:ptCount val="1"/><c:pt idx="0"><c:v>{i+1}</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser>' for i in range(2))
        data = { 'xl/charts/chart1.xml': f'<c:chartSpace xmlns:c="{chartns}" xmlns:a="{a}"><c:chart><c:plotArea><c:barChart><c:barDir val="col"/><c:grouping val="stacked"/>{series}<c:overlap val="100"/><c:axId val="1"/><c:axId val="2"/></c:barChart><c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:axPos val="b"/><c:crossAx val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:axPos val="l"/><c:crossAx val="1"/></c:valAx></c:plotArea></c:chart></c:chartSpace>' }
        kind, target = 'chart', '../charts/chart1.xml'
    else:
        node = '<xdr:pic><xdr:nvPicPr><xdr:cNvPr id="1" name="Cropped"/><xdr:cNvPicPr/></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="object"/><a:srcRect l="50000"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic>'
        def chunk(kind, data):
            return struct.pack('>I',len(data))+kind+data+struct.pack('>I', zlib.crc32(kind+data))
        pixels = b''.join(b'\x00'+b'\xff\x00\x00'*10+b'\x00\x00\xff'*10 for _ in range(20))
        png = b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',20,20,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(pixels))+chunk(b'IEND',b'')
        data = {'xl/media/image1.png':png}
        kind, target = 'image','../media/image1.png'
    data['xl/worksheets/_rels/sheet1.xml.rels'] = f'<Relationships xmlns="{PKG}"><Relationship Id="drawing" Type="{REL}/drawing" Target="../drawings/drawing1.xml"/></Relationships>'
    data['xl/drawings/_rels/drawing1.xml.rels'] = f'<Relationships xmlns="{PKG}"><Relationship Id="object" Type="{REL}/{kind}" Target="{target}"/></Relationships>'
    data['xl/drawings/drawing1.xml'] = f'<xdr:wsDr xmlns:xdr="{xdr}" xmlns:a="{a}" xmlns:r="{REL}"><xdr:oneCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>3</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx="3810000" cy="2540000"/>{node}<xdr:clientData/></xdr:oneCellAnchor></xdr:wsDr>'
    return data

if __name__ == '__main__':
    generate()
