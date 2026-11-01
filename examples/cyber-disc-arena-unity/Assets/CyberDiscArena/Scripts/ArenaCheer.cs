using System.Collections.Generic;
using UnityEngine;

namespace CyberDiscArena
{
    /// 応援の演出。リアクションが届くと、ring の上に応援の文字（ホログラム）を浮かべ、光る粒を散らす。3 秒で消え、同時に
    /// 出すのは 16 個まで（多く届いたら古いものから消す）。
    public sealed class ArenaCheer : MonoBehaviour
    {
        const int Limit = 16;
        const float Seconds = 3f;
        const int Sparks = 8;

        [SerializeField] Material glow;

        readonly List<(Transform root, float born)> shown = new List<(Transform, float)>();
        Font font;

        /// 出した応援の数と、最後の文字（試験・確認用）。
        public int Count { get; private set; }
        public string Last { get; private set; } = "";

        public void Spawn(string label)
        {
            if (shown.Count >= Limit)
            {
                Destroy(shown[0].root.gameObject);
                shown.RemoveAt(0);
            }
            font ??= Resources.GetBuiltinResource<Font>("LegacyRuntime.ttf");
            var root = new GameObject("Cheer").transform;
            root.position = new Vector3(Random.Range(-5f, 5f), 3.5f, Random.Range(-4f, 4f));
            var text = new GameObject("Hologram").AddComponent<TextMesh>();
            text.transform.SetParent(root, false);
            text.font = font;
            text.GetComponent<MeshRenderer>().sharedMaterial = font.material;
            text.text = label;
            text.fontSize = 64;
            text.characterSize = 0.08f;
            text.anchor = TextAnchor.MiddleCenter;
            text.color = new Color(0.5f, 1f, 1f);
            for (var index = 0; index < Sparks; index++)
            {
                var spark = GameObject.CreatePrimitive(PrimitiveType.Cube);
                Destroy(spark.GetComponent<Collider>());
                spark.GetComponent<Renderer>().sharedMaterial = glow;
                spark.transform.SetParent(root, false);
                spark.transform.localScale = Vector3.one * 0.12f;
                spark.name = "Spark";
            }
            shown.Add((root, Time.time));
            Count++;
            Last = label;
        }

        void Update()
        {
            var view = Camera.main != null ? Camera.main.transform.rotation : Quaternion.identity;
            for (var index = shown.Count - 1; index >= 0; index--)
            {
                var (root, born) = shown[index];
                var age = Time.time - born;
                if (age > Seconds)
                {
                    Destroy(root.gameObject);
                    shown.RemoveAt(index);
                    continue;
                }
                root.position += Vector3.up * Time.deltaTime * 0.6f;
                root.GetChild(0).rotation = view;
                // 粒は文字の周りへ円く散る。
                for (var spark = 1; spark < root.childCount; spark++)
                {
                    var angle = spark * Mathf.PI * 2 / Sparks;
                    root.GetChild(spark).localPosition = new Vector3(Mathf.Cos(angle), Mathf.Sin(angle), 0) * (0.5f + age * 1.5f);
                }
            }
        }
    }
}
