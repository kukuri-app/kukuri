// 上の行から並んだ I420（Y・U・V の R8 の 3 枚、BT.601 の limited range）を RGB にする（ADR 0070 §8.3、#1982）。
Shader "Hidden/Kukuri/I420"
{
    Properties
    {
        _MainTex ("Y", 2D) = "black" {}
        _UTex ("U", 2D) = "gray" {}
        _VTex ("V", 2D) = "gray" {}
    }
    SubShader
    {
        Cull Off ZWrite Off ZTest Always
        Pass
        {
            CGPROGRAM
            #pragma vertex vert_img
            #pragma fragment frag
            #include "UnityCG.cginc"

            sampler2D _MainTex;
            sampler2D _UTex;
            sampler2D _VTex;

            fixed4 frag(v2f_img i) : SV_Target
            {
                // texture の 0 行目は上の行なので、上下を返して読む。
                float2 uv = float2(i.uv.x, 1 - i.uv.y);
                float y = (tex2D(_MainTex, uv).r - 16.0 / 255.0) * 1.164;
                float u = tex2D(_UTex, uv).r - 0.5;
                float v = tex2D(_VTex, uv).r - 0.5;
                float3 rgb = saturate(float3(y + 1.596 * v, y - 0.392 * u - 0.813 * v, y + 2.017 * u));
            #ifndef UNITY_COLORSPACE_GAMMA
                rgb = GammaToLinearSpace(rgb);
            #endif
                return fixed4(rgb, 1);
            }
            ENDCG
        }
    }
}
