// wasm-bindgen's Model, made safe to free twice and clear once freed: its
// free() and the Symbol.dispose that `using` calls may both run (an explicit
// free() inside a `using` block), and a freed model's methods throw a
// TypeError that says so, instead of wasm-bindgen's "null pointer passed to
// rust". `__wbg_ptr` is wasm-bindgen 0.2.130's (pinned) handle: 0 once freed.
import { Model } from './litegen_3d_converter.js';

const freed = () => new TypeError('this Model was freed; read the model again with readModel()');
const free = Model.prototype.free;
Model.prototype.free = function () {
  if (this.__wbg_ptr !== 0) free.call(this);
};
Model.prototype[Symbol.dispose] = Model.prototype.free;
for (const [name, d] of Object.entries(Object.getOwnPropertyDescriptors(Model.prototype))) {
  if (name === 'constructor' || name === 'free' || name.startsWith('__')) continue;
  if (d.get) {
    const get = d.get;
    Object.defineProperty(Model.prototype, name, {
      ...d,
      get() {
        if (this.__wbg_ptr === 0) throw freed();
        return get.call(this);
      },
    });
  } else if (typeof d.value === 'function') {
    const method = d.value;
    Model.prototype[name] = function (...args) {
      if (this.__wbg_ptr === 0) throw freed();
      return method.apply(this, args);
    };
  }
}
