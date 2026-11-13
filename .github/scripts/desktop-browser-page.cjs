// Acknowledge real page events and viewport size; never synthesize input.
module.exports = function browserPage(pathname) {
  const ready = pathname === '/desktop-input' ? 'cma-desktop-input-ready' : 'cma-desktop-ready';
  return `<!doctype html><title>cma-desktop-loading</title>
<body style="margin:0;background:#234">
<input id="field" style="position:absolute;left:80px;top:100px;width:300px;height:40px">
<button id="verify" style="position:absolute;left:80px;top:200px;width:250px;height:40px">Verify input</button>
<script>
let stage = '${ready}';
function acknowledge() {
  document.title = stage + '|' + window.innerWidth + 'x' + window.innerHeight;
}
const field = document.getElementById('field');
field.addEventListener('focus', () => { stage = 'cma-input-focused'; acknowledge(); });
field.addEventListener('input', () => { stage = 'cma-input:' + field.value; acknowledge(); });
document.getElementById('verify').addEventListener('click', () => {
  localStorage.setItem('cma-desktop-proof', field.value);
  stage = 'cma-ui:' + field.value;
  acknowledge();
});
window.addEventListener('resize', acknowledge);
fetch('/profile?value=' + encodeURIComponent(localStorage.getItem('cma-desktop-proof') || ''));
acknowledge();
</script>`;
};
