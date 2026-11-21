// The ring pulses each time it is shown (crates/agent-app/src/screen.rs calls window.flash()).
window.flash = () => {
  const ring = document.getElementById('ring');
  ring.classList.remove('flash');
  void ring.offsetWidth;
  ring.classList.add('flash');
};
