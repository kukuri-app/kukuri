// kukuri LP (#1043)。JS が無くても全リンクが使える。JS は案内を OS に合わせるだけ。
(function () {
  'use strict';

  var root = document.documentElement;
  var ua = navigator.userAgent || '';
  var platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || '';

  var isMobile =
    /Android|iPhone|iPad|iPod|Mobile/i.test(ua) ||
    (navigator.userAgentData && navigator.userAgentData.mobile === true);

  // PC では見ている OS のダウンロードを、スマートフォンではブラウザ版を先頭の主ボタンにする。
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

  document.querySelectorAll('[data-download-group]').forEach(function (group) {
    var preferred = group.querySelector('[data-os-target="' + os + '"]');
    if (preferred) {
      group.querySelectorAll('[data-os-target]').forEach(function (button) {
        button.classList.remove('button-primary');
        button.classList.add('button-secondary');
      });
      preferred.classList.remove('button-secondary');
      preferred.classList.add('button-primary');
      group.insertBefore(preferred, group.firstChild);
    }
  });

  document.querySelectorAll('[data-mac-note]').forEach(function (note) {
    note.hidden = os !== 'mac';
  });
})();
