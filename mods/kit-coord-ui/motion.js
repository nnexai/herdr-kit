import { exec } from 'herdr';

// Herdr has no motion-preference API. Probe the OS once per opened log; never change it.
export async function systemReducedMotion() {
  try {
    const system = await exec('uname',['-s'],{timeout:2000});
    if (system.code !== 0) return false;
    const linux = system.stdout.trim() === 'Linux';
    if (!linux && system.stdout.trim() !== 'Darwin') return false;
    const value = await exec(linux ? 'gsettings' : 'defaults',linux
      ? ['get','org.gnome.desktop.interface','enable-animations']
      : ['read','com.apple.universalaccess','reduceMotion'],{timeout:2000});
    return value.code === 0 && (linux ? value.stdout.trim() === 'false' : value.stdout.trim() === '1');
  } catch (_) { return false; }
}
