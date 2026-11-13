// Page acknowledgements distinguish navigation and real screen input stages.
module.exports = function browserPage(pathname) {
  const ready = pathname === '/desktop-input' ? 'cma-desktop-input-ready' : 'cma-desktop-ready';
  return `<!doctype html><title>cma-desktop-loading</title>
<body style="margin:0;background:#234">
<input id="field" style="position:absolute;left:80px;top:100px;width:300px;height:40px">
<button id="verify" style="position:absolute;left:80px;top:200px;width:250px;height:40px">Verify input</button>
<script>
const field = document.getElementById('field');
field.addEventListener('focus', () => { document.title = 'cma-input-focused'; });
field.addEventListener('input', () => { document.title = 'cma-input:' + field.value; });
document.getElementById('verify').addEventListener('click', () => {
  localStorage.setItem('cma-desktop-proof', field.value);
  document.title = 'cma-ui:' + field.value;
});
window.addEventListener('resize', () => {
  if (document.title === 'cma-desktop-input-ready' && window.innerWidth === 1280 && window.innerHeight === 800)
    document.title = 'cma-desktop-fullscreen';
});
fetch('/profile?value=' + encodeURIComponent(localStorage.getItem('cma-desktop-proof') || ''));
document.title = '${ready}';
</script>`;
};
