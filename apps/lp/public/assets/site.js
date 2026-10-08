// kukuri LP (#1043, #1669)。JS が無くても全リンクが使え、主ボタンはブラウザ版になる。JS は案内を OS に合わせるだけ。
(function () {
  'use strict';

  var root = document.documentElement;
  var ua = navigator.userAgent || '';
  var platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || '';

  var isMobile =
    /Android|iPhone|iPad|iPod|Mobile/i.test(ua) ||
    (navigator.userAgentData && navigator.userAgentData.mobile === true);

  var os = isMobile
    ? 'mobile'
    : /Win/i.test(platform) || /Windows/i.test(ua)
      ? 'windows'
      : /Linux/i.test(platform) || /Linux/i.test(ua)
        ? 'linux'
        : /Mac/i.test(platform) || /Mac OS/i.test(ua)
          ? 'mac'
          : 'other';
  root.setAttribute('data-os', os);

  // アプリがある Windows・Linux はその配布物を、それ以外（Mac・スマートフォン・判定できない OS）はブラウザ版を主ボタンにする。
  var target = os === 'windows' || os === 'linux' ? os : 'web';
  document.querySelectorAll('[data-download-group]').forEach(function (group) {
    var preferred = group.querySelector('[data-os-target="' + target + '"]');
    group.querySelectorAll('[data-os-target]').forEach(function (button) {
      button.classList.toggle('button-primary', button === preferred);
      button.classList.toggle('button-secondary', button !== preferred);
    });
    group.insertBefore(preferred, group.firstChild);
  });

  document.querySelectorAll('[data-mac-note]').forEach(function (note) {
    note.hidden = os !== 'mac';
  });

  // 「Linux版をダウンロード」の行き先は閉じた詳細なので、開いてから移る。
  var downloads = document.getElementById('download');
  document.querySelectorAll('a[href="#download"]').forEach(function (link) {
    link.addEventListener('click', function () {
      downloads.open = true;
    });
  });
})();
