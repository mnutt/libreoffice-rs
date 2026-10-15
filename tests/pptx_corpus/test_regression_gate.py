import unittest
from regression_gate import assess


class RegressionGateTest(unittest.TestCase):
    def test_bad_later_page_cannot_be_hidden_by_better_first_page(self):
        empty = dict(cases=[])
        before = dict(cases=[dict(id='deck',pages=[dict(page=1,rmse=0.5),dict(page=7,rmse=0.1)])])
        after = dict(cases=[dict(id='deck',pages=[dict(page=1,rmse=0.0),dict(page=7,rmse=0.2)])])
        result=assess(empty,empty,before,after)
        self.assertEqual([(f['kind'],f['page']) for f in result['failures']],[('pixels',7)])

    def test_losing_comparability_is_a_failure(self):
        empty=dict(cases=[])
        before=dict(cases=[dict(id='deck',pages=[dict(page=3,rmse=0.1)])])
        result=assess(empty,empty,before,empty)
        self.assertEqual(result['failures'][0]['kind'],'lost_visual_comparison')


if __name__ == '__main__':unittest.main()
