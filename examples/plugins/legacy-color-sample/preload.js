window.exports = {
  color_pick: {
    mode: 'list',
    args: {
      enter(_action, callbackSetList) {
        callbackSetList([
          { title: '#0F172A', description: 'Slate 900' },
          { title: '#38BDF8', description: 'Sky 400' },
          { title: '#22C55E', description: 'Emerald 500' }
        ]);
      },
      search(_action, searchWord, callbackSetList) {
        const q = String(searchWord || '').toLowerCase();
        const items = [
          { title: '#0F172A', description: 'Slate 900' },
          { title: '#38BDF8', description: 'Sky 400' },
          { title: '#22C55E', description: 'Emerald 500' }
        ].filter((item) => item.title.toLowerCase().includes(q) || item.description.toLowerCase().includes(q));
        callbackSetList(items);
      },
      select(_action, itemData) {
        if (window.aitools && itemData) {
          window.aitools.copyText(itemData.title);
          window.aitools.outPlugin();
        }
      }
    }
  }
};
