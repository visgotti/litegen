#!/usr/bin/env python3
"""Hand-built reference 3MF (core 1.4 + materials 1.2.1), stdlib only.

objects: 2 = textured cube (texture2dgroup), 3 = vertex-coloured tetra (colorgroup),
         4 = basematerials tetra (one triangle overridden), 10 = components of 2,3,4
build:   item 10 with a translation transform
"""
import os, sys, zipfile

out_path, png_path = sys.argv[1], sys.argv[2]

V = [(0, 0, 0), (20, 0, 0), (20, 20, 0), (0, 20, 0), (0, 0, 20), (20, 0, 20), (20, 20, 20), (0, 20, 20)]
QUADS = [(0, 1, 5, 4), (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7), (4, 5, 6, 7), (1, 0, 3, 2)]  # bl br tr tl from outside
TET = [(0, 0, 0), (20, 0, 0), (10, 20, 0), (10, 7, 20)]
TET_TRIS = [(0, 2, 1), (0, 1, 3), (1, 2, 3), (2, 0, 3)]

def verts(vs):
    return '\n'.join(f'          <vertex x="{x}" y="{y}" z="{z}"/>' for x, y, z in vs)

cube_tris = []
for bl, br, tr, tl in QUADS:
    cube_tris.append(f'          <triangle v1="{bl}" v2="{br}" v3="{tr}" p1="0" p2="1" p3="2"/>')
    cube_tris.append(f'          <triangle v1="{bl}" v2="{tr}" v3="{tl}" p1="0" p2="2" p3="3"/>')
tet_col = [f'          <triangle v1="{a}" v2="{b}" v3="{c}" p1="{a}" p2="{b}" p3="{c}"/>' for a, b, c in TET_TRIS]
tet_base = [f'          <triangle v1="{a}" v2="{b}" v3="{c}"/>' for a, b, c in TET_TRIS[:3]] + \
           [f'          <triangle v1="{a}" v2="{b}" v3="{c}" pid="1" p1="1"/>' for a, b, c in TET_TRIS[3:]]

model = f'''<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US"
       xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
       xmlns:m="http://schemas.microsoft.com/3dmanufacturing/material/2015/02"
       xmlns:lg="urn:example:litegen">
  <metadata name="Title">litegen reference</metadata>
  <metadata name="Application">make_ref_3mf.py</metadata>
  <metadata name="lg:SourceFormat">glTF 2.0</metadata>
  <resources>
    <basematerials id="1">
      <base name="Red" displaycolor="#FF0000FF"/>
      <base name="White" displaycolor="#FFFFFF"/>
    </basematerials>
    <m:colorgroup id="5">
      <m:color color="#FF0000"/>
      <m:color color="#00FF00"/>
      <m:color color="#0000FF"/>
      <m:color color="#FFFF00"/>
    </m:colorgroup>
    <m:texture2d id="6" path="/3D/Texture/basecolor.png" contenttype="image/png" tilestyleu="wrap" tilestylev="wrap" filter="auto"/>
    <m:texture2dgroup id="7" texid="6">
      <m:tex2coord u="0" v="0"/>
      <m:tex2coord u="1" v="0"/>
      <m:tex2coord u="1" v="1"/>
      <m:tex2coord u="0" v="1"/>
    </m:texture2dgroup>
    <object id="2" type="model" name="TexturedCube" pid="7" pindex="0">
      <mesh>
        <vertices>
{verts(V)}
        </vertices>
        <triangles>
{chr(10).join(cube_tris)}
        </triangles>
      </mesh>
    </object>
    <object id="3" type="model" name="ColoredTetra" pid="5" pindex="0">
      <mesh>
        <vertices>
{verts(TET)}
        </vertices>
        <triangles>
{chr(10).join(tet_col)}
        </triangles>
      </mesh>
    </object>
    <object id="4" type="model" name="BaseMatTetra" pid="1" pindex="0">
      <mesh>
        <vertices>
{verts(TET)}
        </vertices>
        <triangles>
{chr(10).join(tet_base)}
        </triangles>
      </mesh>
    </object>
    <object id="10" type="model" name="Assembly">
      <components>
        <component objectid="2"/>
        <component objectid="3" transform="1 0 0 0 1 0 0 0 1 30 0 0"/>
        <component objectid="4" transform="1 0 0 0 1 0 0 0 1 60 0 0"/>
      </components>
    </object>
  </resources>
  <build>
    <item objectid="10" transform="1 0 0 0 1 0 0 0 1 50 50 0"/>
  </build>
</model>
'''

content_types = '''<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
  <Default Extension="png" ContentType="image/png"/>
</Types>
'''
root_rels = '''<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rel0" Target="/3D/3dmodel.model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
  <Relationship Id="rel1" Target="/Metadata/thumbnail.png" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail"/>
</Relationships>
'''
model_rels = '''<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="tex0" Target="/3D/Texture/basecolor.png" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dtexture"/>
</Relationships>
'''
png = open(png_path, 'rb').read()
with zipfile.ZipFile(out_path, 'w', zipfile.ZIP_DEFLATED) as z:
    z.writestr('[Content_Types].xml', content_types)
    z.writestr('_rels/.rels', root_rels)
    z.writestr('3D/3dmodel.model', model)
    z.writestr('3D/_rels/3dmodel.model.rels', model_rels)
    z.writestr('3D/Texture/basecolor.png', png)
    z.writestr('Metadata/thumbnail.png', png)
open(out_path.replace('.3mf', '.model.xml'), 'w').write(model)
print('ok', out_path)
