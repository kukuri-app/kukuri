using System;
using UnityEngine;
using UnityEngine.UI;

namespace Kukuri.GameSdk.Social
{
    /// 掲示板の 1 行。見た目は prefab で差し替えられる。
    public sealed class KukuriBoardItem : MonoBehaviour
    {
        [SerializeField] Text authorText;
        [SerializeField] Text contentText;
        [SerializeField] Button openButton;

        string threadId;
        Action<string> open;

        void Awake()
        {
            if (openButton != null)
                openButton.onClick.AddListener(() => open?.Invoke(threadId));
        }

        public string Author => authorText != null ? authorText.text : "";
        public string Content => contentText != null ? contentText.text : "";

        internal void Show(BoardPost post, bool openable, Action<string> onOpen)
        {
            threadId = post.ThreadId;
            open = onOpen;
            var author = string.IsNullOrEmpty(post.AuthorName)
                ? post.AuthorPubkey.Substring(0, Math.Min(12, post.AuthorPubkey.Length))
                : post.AuthorName;
            if (authorText != null)
                authorText.text = $"{author}・{post.Audience}" + (post.ReplyTo != null ? "・返信" : "");
            if (contentText != null)
                contentText.text = post.Content;
            if (openButton != null)
                openButton.gameObject.SetActive(openable);
        }
    }
}
