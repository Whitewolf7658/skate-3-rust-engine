(() => {
    const canvas = document.getElementById('bevy');
    const overlay = document.getElementById('controls-guide');
    const open = document.getElementById('controls-open');
    const close = document.getElementById('controls-close');
    let dragging = false;
    let position = [0, 0];
    let previous = null;

    window.skateMouseStick = [0, 0];

    const publish = () => {
        window.skateMouseStick = position.slice();
    };

    const reset = () => {
        dragging = false;
        previous = null;
        position = [0, 0];
        publish();
    };

    canvas.addEventListener('contextmenu', event => event.preventDefault());

    canvas.addEventListener('pointerdown', event => {
        if (event.button !== 2 || overlay.classList.contains('open')) return;
        dragging = true;
        previous = [event.clientX, event.clientY];
        position = [0, 0];
        publish();
        canvas.setPointerCapture?.(event.pointerId);
        event.preventDefault();
    });

    canvas.addEventListener('pointermove', event => {
        if (!dragging || !previous) return;
        const dx = event.clientX - previous[0];
        const dy = event.clientY - previous[1];
        previous = [event.clientX, event.clientY];
        const amount = Math.hypot(dx, dy);
        if (amount < 2) return;
        const scale = Math.min(1, amount / 28);
        position = [dx / amount * scale, -dy / amount * scale];
        publish();
    });

    canvas.addEventListener('pointerup', event => {
        if (event.button === 2) reset();
    });
    canvas.addEventListener('pointercancel', reset);
    window.addEventListener('blur', reset);

    const show = visible => {
        overlay.classList.toggle('open', visible);
        overlay.setAttribute('aria-hidden', String(!visible));
        if (visible) {
            reset();
            close.focus();
        } else {
            canvas.focus();
        }
    };

    open.addEventListener('click', () => show(true));
    close.addEventListener('click', () => show(false));
    overlay.addEventListener('click', event => {
        if (event.target === overlay) show(false);
    });
    window.addEventListener('keydown', event => {
        if (event.code === 'F1') {
            event.preventDefault();
            event.stopImmediatePropagation();
            show(!overlay.classList.contains('open'));
        } else if (event.code === 'Escape' && overlay.classList.contains('open')) {
            event.preventDefault();
            event.stopImmediatePropagation();
            show(false);
        }
    }, true);
})();
