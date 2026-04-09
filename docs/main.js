(function() {
  if (!('IntersectionObserver' in window)) {
    document.querySelectorAll('.reveal').forEach(function(el) {
      el.classList.add('visible');
    });
    return;
  }

  var observer = new IntersectionObserver(function(entries) {
    entries.forEach(function(entry) {
      if (entry.isIntersecting) {
        entry.target.classList.add('visible');
        observer.unobserve(entry.target);
      }
    });
  }, {
    threshold: 0.12,
    rootMargin: '0px 0px -40px 0px'
  });

  document.querySelectorAll('.reveal').forEach(function(el) {
    observer.observe(el);
  });
})();

/* Obfuscate — assemble split data attributes for bot protection */
document.querySelectorAll('.obf-p,.obf-a,.obf-b').forEach(function (el) {
  el.textContent = (el.dataset.a || '') + (el.dataset.b || '') + (el.dataset.c || '');
});
