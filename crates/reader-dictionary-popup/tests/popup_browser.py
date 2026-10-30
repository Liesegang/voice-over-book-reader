"""Optional browser regression suite for the real emitted popup shell.

Prepare HTML with `cargo run -p reader-dictionary-popup --example render_fixture > FILE`,
then run `python crates/reader-dictionary-popup/tests/popup_browser.py --html FILE`.
Requires Python Playwright and its Chromium browser; uses only intercepted local fixtures.
"""
import argparse
import json
from pathlib import Path
import unittest
from playwright.sync_api import sync_playwright

PARSER = argparse.ArgumentParser()
PARSER.add_argument('--html', required=True, type=Path)
PARSER.add_argument('--channel', default=None, help='Optional installed Chromium channel, e.g. msedge')
ARGS, REST = PARSER.parse_known_args()
SOURCE = Path(__file__).resolve().parents[1] / 'src'

class ContinuousDictionaryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.driver = sync_playwright().start()
        cls.browser = cls.driver.chromium.launch(headless=True,channel=ARGS.channel)

    @classmethod
    def tearDownClass(cls):
        cls.browser.close()
        cls.driver.stop()

    def setUp(self):
        self.page = self.browser.new_page(viewport={'width':560, 'height':420}, reduced_motion='reduce')
        self.errors = []
        self.page.on('pageerror', lambda error: self.errors.append(str(error)))
        self.page.add_init_script('window.actions=[];window.ipc={postMessage:v=>window.actions.push(JSON.parse(v))};')
        bridge = (SOURCE / 'entry.js').read_text(encoding='utf-8-sig')
        def respond(route):
            path = route.request.url.split('/')
            identity = path[3]
            ordinal = path[-1]
            height = 32000 if ordinal == '999' else 600
            body = f'''<!doctype html><html><head><style>
                html,body{{margin:0;padding:0;height:auto;min-height:0;overflow-y:hidden}}
                body{{display:flow-root}}.definition{{height:{height}px;color:rgb(90,30,20)}}
                </style></head><body><div class="definition">原定義 {ordinal}<a href="entry://犬">犬</a></div>
                <script>window.authorScriptRuns=(window.authorScriptRuns||0)+1;
                window.dictionaryIdentity={json.dumps(identity)};{bridge}</script></body></html>'''
            route.fulfill(status=200,content_type='text/html; charset=utf-8',body=body)
        self.page.route('**/dictionary.localhost/**', respond)
        self.page.route('**://dictionary.localhost/**', respond)
        self.page.goto(ARGS.html.resolve().as_uri())
        self.page.wait_for_load_state('networkidle')

    def tearDown(self):
        self.assertEqual(self.errors, [])
        self.page.close()

    def state(self, count=12, entries=True):
        return {'query':'猫', 'selected':None, 'theme':'light','accent':'2563eb','pinned':False,
                'paper':None,'ink':None,'diagnostics':[],
                'sections':[{'id':f'{n:064x}', 'title':f'辞書 {n} 非常に長い辞書名',
                             'count':1 if entries else None,
                             'entries':[{'ordinal':n+10,'headword':'猫','src':f'http://dictionary.localhost/{n:064x}/__entry/{n+10}'}] if entries else []}
                            for n in range(count)]}

    def apply(self, state):
        self.page.evaluate('(state)=>window.dictionaryView.apply(state)', state)
        self.page.wait_for_timeout(80)

    def test_names_pending_and_zero_hits_exist_at_narrow_width(self):
        self.page.set_viewport_size({'width':320,'height':240})
        self.assertEqual(self.page.locator('.dictionary-section').count(),3)
        self.assertEqual(self.page.get_by_role('button').filter(has=self.page.locator('.index-title')).count(),3)
        self.assertIn('順に検索しています',self.page.locator('#results').inner_text())
        self.assertIn('該当する項目はありません',self.page.locator('#results').inner_text())
        dims=self.page.evaluate('({overflow:document.documentElement.scrollWidth>innerWidth, results:document.querySelector("#results").getBoundingClientRect().right, index:document.querySelector("#dictionary-index").getBoundingClientRect().left})')
        self.assertFalse(dims['overflow'])
        self.assertLessEqual(dims['results'],dims['index']+1)
        self.assertEqual(self.page.get_by_role('tab').count(),0)

    def test_large_catalog_defers_frames_and_index_supports_keyboard_scrollspy(self):
        state=self.state(120)
        self.apply(state)
        self.assertEqual(self.page.locator('.dictionary-section').count(),120)
        self.assertLess(self.page.locator('iframe').count(),10)
        button=self.page.locator('.index-link').nth(60)
        button.click()
        self.page.wait_for_timeout(100)
        self.assertEqual(button.get_attribute('aria-current'),'location')
        self.assertGreater(self.page.locator('#results').evaluate('(e)=>e.scrollTop'),10000)
        button.press('ArrowDown')
        self.page.wait_for_timeout(80)
        self.assertEqual(self.page.locator('.index-link').nth(61).get_attribute('aria-current'),'location')
        self.assertTrue(self.page.evaluate('window.actions.some(a=>a.action==="dictionary")'))
        self.assertLessEqual(self.page.locator('iframe').count(),24)

    def test_incremental_results_preserve_visible_frame_and_scroll_anchor(self):
        state=self.state(8)
        state['sections'][0]['entries']=[];state['sections'][0]['count']=None
        self.apply(state)
        self.page.locator('.index-link').nth(3).click()
        self.page.wait_for_timeout(200)
        card=self.page.locator('.dictionary-section').nth(3).locator('.entry')
        self.page.wait_for_function('document.querySelectorAll(".dictionary-section")[3].querySelector(".entry.loaded")')
        self.assertEqual(card.locator('iframe').content_frame.locator('body').evaluate('()=>window.authorScriptRuns'),1)
        before=card.bounding_box()['y']
        card.locator('iframe').evaluate('(e)=>e.dataset.preserved="yes"')
        state['sections'][0]['entries']=[{'ordinal':999,'headword':'長文','src':f'http://dictionary.localhost/{0:064x}/__entry/999'}]
        state['sections'][0]['count']=1
        self.apply(state)
        self.assertEqual(card.locator('iframe').get_attribute('data-preserved'),'yes')
        self.assertAlmostEqual(card.bounding_box()['y'],before,delta=2)

    def test_long_article_has_no_twenty_thousand_pixel_truncation_and_wheel_chains(self):
        state=self.state(1)
        state['sections'][0]['entries'][0]['ordinal']=999
        state['sections'][0]['entries'][0]['src']=f'http://dictionary.localhost/{0:064x}/__entry/999'
        self.apply(state)
        self.page.wait_for_function('document.querySelector("iframe")?.getBoundingClientRect().height>30000')
        frame=self.page.locator('iframe').content_frame
        self.assertEqual(frame.locator('.definition').evaluate('(e)=>getComputedStyle(e).color'),'rgb(90, 30, 20)')
        self.page.mouse.move(80,150);self.page.mouse.wheel(0,240)
        self.page.wait_for_timeout(120)
        self.assertGreater(self.page.locator('#results').evaluate('(e)=>e.scrollTop'),100)
        self.assertEqual(frame.locator('body').evaluate('()=>window.scrollY'),0)
        frame.get_by_text('犬',exact=True).click()
        self.assertTrue(self.page.evaluate('window.actions.some(a=>a.action==="entry"&&a.query==="犬")'))

    def test_custom_paper_ink_and_forged_cross_frame_message(self):
        state=self.state(2);state['paper']='002B36';state['ink']='839496';self.apply(state)
        self.assertEqual(self.page.locator('body').evaluate('(e)=>getComputedStyle(e).backgroundColor'),'rgb(0, 43, 54)')
        self.assertEqual(self.page.locator('body').evaluate('(e)=>getComputedStyle(e).color'),'rgb(131, 148, 150)')
        count=self.page.evaluate('window.actions.length')
        self.page.evaluate('window.postMessage({action:"entry",dictionary_id:"x",query:"forged"},"*")')
        self.page.wait_for_timeout(40)
        self.assertEqual(self.page.evaluate('window.actions.length'),count)
        self.page.locator('#pin').click()
        self.assertEqual(self.page.evaluate('window.actions.at(-1).action'),'pin')
        self.page.keyboard.press('Escape')
        self.assertEqual(self.page.evaluate('window.actions.at(-1).action'),'close')

if __name__=='__main__':
    unittest.main(argv=[__file__,*REST])
