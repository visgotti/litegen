#!/bin/bash
# Negative tests for usdchecker --arkit (Apple USD Tools 0.25.2)
D="${D:-$(cd "$(dirname "$0")/.." && pwd)}"
N=$D/neg; rm -rf $N; mkdir -p $N/textures; cd $N
cp $D/usd/textures/basecolor.png textures/
CHK=$D/scripts/chk.sh

# 1. deflate-compressed usdz, 2. stored but unaligned usdz (python zipfile)
python3 -I - <<'PY'
import zipfile
for name, comp in (('deflate.usdz', zipfile.ZIP_DEFLATED), ('unaligned.usdz', zipfile.ZIP_STORED)):
    with zipfile.ZipFile(name, 'w', comp) as z:
        z.write('../usd/quad.usda', 'quad.usda')
        for t in ('basecolor.png', 'mr.png', 'normal.png'):
            z.write('../usd/textures/' + t, 'textures/' + t)
PY
for f in deflate.usdz unaligned.usdz; do echo "### $f"; $CHK --arkit $f; done

# 3. texture first in package (root layer not first)
python3 -I - <<'PY'
import subprocess
PY
echo "### png-first (usdzip with png listed first)"
cp ../usd/quad.usda . ; mkdir -p textures; cp ../usd/textures/*.png textures/
usdzip pngfirst.usdz textures/basecolor.png quad.usda textures/mr.png textures/normal.png 2>&1 | grep -v "Coding Error"
usdzip -l - pngfirst.usdz 2>&1 | grep -v "Coding Error"
$CHK --arkit pngfirst.usdz

# 4. stage with no metadata, PointInstancer, custom shader id, tga texture, joints child-before-parent
cat > misc.usda <<'EOF'
#usda 1.0

def Xform "Root"
{
    def PointInstancer "PI"
    {
        point3f[] positions = [(0, 0, 0)]
        int[] protoIndices = [0]
        rel prototypes = </Root/PI/Protos/Cube>
        def Scope "Protos"
        {
            def Cube "Cube"
            {
            }
        }
    }

    def Mesh "M" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
        rel material:binding = </Root/Mat>
    }

    def Material "Mat"
    {
        token outputs:surface.connect = </Root/Mat/S.outputs:surface>
        def Shader "S"
        {
            uniform token info:id = "MyCustomShader"
            token outputs:surface
        }
        def Shader "T"
        {
            uniform token info:id = "UsdUVTexture"
            asset inputs:file = @textures/foo.tga@
            float3 outputs:rgb
        }
    }

    def SkelRoot "Rig"
    {
        def Skeleton "Skel"
        {
            uniform token[] joints = ["A/B", "A"]
            uniform matrix4d[] bindTransforms = [( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1) ), ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1) )]
            uniform matrix4d[] restTransforms = [( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1) ), ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1) )]
        }
    }
}
EOF
printf 'TGA' > textures/foo.tga
echo "### misc.usda (no layer metadata, PointInstancer, custom shader, tga, bad joint order)"
$CHK --arkit misc.usda
usdzip misc.usdz misc.usda textures/foo.tga 2>&1 | grep -v "Coding Error"
echo "### misc.usdz"
$CHK --arkit misc.usdz

# 5. varname authored as token instead of string; MaterialBindingAPI missing
cat > types.usda <<'EOF'
#usda 1.0
(
    defaultPrim = "Root"
    metersPerUnit = 1
    upAxis = "Y"
)
def Xform "Root"
{
    def Mesh "M"
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
        rel material:binding = </Root/Mat>
    }
    def Material "Mat"
    {
        token outputs:surface.connect = </Root/Mat/PBR.outputs:surface>
        def Shader "PBR"
        {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor.connect = </Root/Mat/Tex.outputs:rgb>
            float3 inputs:emissiveColor = (0, 0, 0)
            token outputs:surface
        }
        def Shader "Reader"
        {
            uniform token info:id = "UsdPrimvarReader_float2"
            token inputs:varname = "st"
            float2 outputs:result
        }
        def Shader "Tex"
        {
            uniform token info:id = "UsdUVTexture"
            asset inputs:file = @textures/basecolor.png@
            float2 inputs:st.connect = </Root/Mat/Reader.outputs:result>
            float3 outputs:rgb
        }
    }
}
EOF
echo "### types.usda (token varname, float3 emissiveColor, no MaterialBindingAPI)"
$CHK --arkit types.usda
