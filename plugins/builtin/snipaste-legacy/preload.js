window.exports = {
  'snipaste-ocr': {
    mode: 'list',
    args: {
      enter(_action, callbackSetList) {
        callbackSetList([
          { title: 'Capture Region -> OCR', description: 'Extract text from selected screen area', code: 'capture-ocr' },
          { title: 'Decode QR / Barcode', description: 'Scan clipboard image for QR payloads', code: 'decode-qr' }
        ]);
      },
      search(_action, searchWord, callbackSetList) {
        const q = String(searchWord || '').toLowerCase();
        const items = [
          { title: 'Capture Region -> OCR', description: 'Extract text from selected screen area', code: 'capture-ocr' },
          { title: 'Decode QR / Barcode', description: 'Scan clipboard image for QR payloads', code: 'decode-qr' }
        ].filter((it) => it.title.toLowerCase().includes(q) || it.description.toLowerCase().includes(q));
        callbackSetList(items);
      },
      select(_action, itemData) {
        if (window.utools && itemData) {
          window.utools.copyText(`[snipaste-ocr:${itemData.code}]`);
          window.utools.outPlugin();
        }
      }
    }
  }
};
